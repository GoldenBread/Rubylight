//! Temporary exclusive/primary/isolated layouts with shared and crash restoration.

mod activation;
mod lease;
mod recall;
mod remembered;
mod wake;
mod wake_follow;

pub use activation::Activation;
use activation::activation_state;
pub use lease::Lease;
use lease::failed_repair;
#[cfg(test)]
use lease::{EndLeaves, reapplied_target, repair_then_lay_out, what_an_end_leaves};
pub use recall::undo_recall_for_remote_monitor;
#[cfg(test)]
use recall::{
    after_departure, departure_repair, entry_layout, put_entry_back, remote_monitor_baseline,
    remote_monitor_repair,
};
use recall::{
    forget_saved_layout, isolated, persist, recall_to_undo, repair_saves,
    undo_recall_after_departure,
};
pub use remembered::initialize as initialize_remembered;
pub(crate) use wake::present_layout;
use wake::{
    LeftWatch, Wake, WakeEnd, WakeLook, WakeLooks, all_listed, displays_to_watch, start_left_watch,
    start_wake_watch, strays, switch_off_strays,
};
#[cfg(test)]
use wake::{
    WAKE_POLL, WAKE_RETURN, WAKE_WATCH, WakeWatcher, WatchStart, WatchStop, after_wake,
    displays_it_could_switch_on, displays_to_wake, entry_goes_unwatched, layout_is_back,
    left_watch, run_wake_watch, settled_step, strays_off_saves, watch_step, watch_wake,
    woken_to_watch,
};
pub use wake::{WakeWatchOff, close_wake_watch, hold_off_wake_watch, wake_watch_settling};
pub use wake_follow::wake_layouts;
#[cfg(test)]
use wake_follow::{
    FOLLOW_TRIES, WakeRepair, layout_around_streams, pass_after_wake, wake_layout_wrong,
};
use wake_follow::{
    FollowStep, WakeFollow, beside_the_streams, joins_a_cut_watch, lay_out_after_wake, wake_follow,
};

use crate::display::{Snapshot, Topology};
use anyhow::Result;
use butterpollo_media_policy::{display_policy::Arrangement, topology::Node};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};

#[derive(Default)]
struct State {
    users: usize,
    key: String,
    before: Option<Snapshot>,
    applied: Vec<Node>,
    /// Streamed displays in the order their streams arrived, with their lease
    /// counts. The first is laid out as for a single stream.
    owners: Vec<(String, usize)>,
    arrangement: Option<Arrangement>,
    retained: Vec<String>,
    /// The layout is saved to the Windows display database (see `persist`).
    persisted: bool,
    /// The owners that layout is saved for: virtual displays made for streaming.
    /// A remote monitor or a physical display among the owners has none, and
    /// without one a save would change an entry nothing puts back.
    persists_for: Vec<String>,
    /// This layout took up what a stopped watch left ([`WakeFollow`]).
    following: Option<WakeFollow>,
    /// The paired device each stream's display streams to, by display: a device's two
    /// screens (Dual screen mode) have the same one. Only the owners' entries count.
    devices: std::collections::BTreeMap<String, String>,
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}
/// Whether the layout just applied makes `target` the primary display. A layout with several
/// streams' displays makes only the first owner's primary (`Arrangement::compose_all`), so a
/// stream that joined another's display, another device or a device's second screen has no
/// primary of its own to watch.
fn applied_primary_for(applied: &[Node], target: &str) -> bool {
    applied
        .iter()
        .any(|node| node.active && node.primary && node.device_id == target)
}
pub fn matches(nodes: &[Node], applied: &[Node]) -> bool {
    matches_connected(nodes, applied, None)
}
/// Like `matches`, but a display that is no longer connected (a TV switched
/// off at the wall, an unplugged monitor) does not count as a change by the
/// user: the rest of the layout is still the one the stream applied.
fn matches_connected(
    nodes: &[Node],
    applied: &[Node],
    connected: Option<&std::collections::BTreeSet<String>>,
) -> bool {
    let present = |id: &str| connected.is_none_or(|c| c.contains(id));
    let active: std::collections::BTreeSet<_> = nodes
        .iter()
        .filter(|n| n.active)
        .map(|n| n.device_id.as_str())
        .collect();
    let expected: std::collections::BTreeSet<_> = applied
        .iter()
        .filter(|n| n.active && present(&n.device_id))
        .map(|n| n.device_id.as_str())
        .collect();
    active == expected
        && nodes.iter().all(|n| {
            applied
                .iter()
                .any(|a| a.device_id == n.device_id && a.desired_position == n.desired_position)
        })
}
/// Whether the displays still have the layout a stream applied.
pub fn unchanged(applied: &[Node]) -> Result<bool> {
    let current = Topology::query()?.nodes()?;
    let connected = Topology::query_all()?
        .monitors()
        .into_iter()
        .map(|m| m.device_id)
        .collect();
    Ok(matches_connected(&current, applied, Some(&connected)))
}
/// An arrangement that disabled an original display must put the original
/// layout back even if Win+P or Windows recovery changed it in the meantime.
/// Non-exclusive arrangements continue to preserve later user changes.
pub fn disabled_original_displays(before: &[Node], applied: &[Node]) -> bool {
    before.iter().filter(|node| node.active).any(|original| {
        !applied
            .iter()
            .any(|node| node.device_id == original.device_id && node.active)
    })
}
pub fn restore_arrangement(before: &Snapshot, applied: &[Node]) -> Result<()> {
    if disabled_original_displays(&before.nodes, applied) || unchanged(applied)? {
        before.restore()?;
    }
    Ok(())
}
/// Tell the layouts streams hold that a remote monitor's display was
/// switched on (`on`) or is about to be removed. Otherwise a stream's end
/// finds a layout it did not apply, takes it for the user's own and leaves
/// the physical displays off, and a restore would switch the remote monitor
/// off. `output`: its GDI name or device id.
pub fn retain(output: &str, on: bool) -> Result<()> {
    let arranged = crate::lock_unpoisoned(state()).users != 0;
    let activated = crate::lock_unpoisoned(activation_state()).users != 0;
    if !arranged && !activated {
        return Ok(());
    }
    let id = crate::display::monitors()
        .ok()
        .and_then(|all| all.into_iter().find(|m| m.matches(output)))
        .map_or_else(|| output.to_owned(), |m| m.device_id);
    let current = if on {
        Topology::query()?.nodes()?
    } else {
        Vec::new()
    };
    let mut result = Ok(());
    {
        let mut state = crate::lock_unpoisoned(state());
        if retain_in(&mut state, &current, &id, on)
            && let Some(before) = &state.before
        {
            result =
                crate::display_recovery::arrangement(Some((before.clone(), state.applied.clone())));
        }
    }
    let mut state = crate::lock_unpoisoned(activation_state());
    if retain_in(&mut state, &current, &id, on)
        && let Some(before) = &state.before
    {
        result = result.and(crate::display_recovery::activation(Some((
            before.clone(),
            state.applied.clone(),
        ))));
    }
    result
}
/// Record `id` in a held layout. Switched on, it is expected active where it
/// is now and restored active with the layout from before the stream; going
/// away, it is only no longer kept on. Its node stays: a display that is gone
/// does not count as a change, and one still there until its lease ends
/// must not either. True when the recorded layout changed.
fn retain_in(state: &mut State, current: &[Node], id: &str, on: bool) -> bool {
    if state.users == 0 {
        return false;
    }
    if !on {
        state.retained.retain(|r| r != id);
        return false;
    }
    let Some(node) = current.iter().find(|n| n.device_id == id) else {
        return false;
    };
    match state.applied.iter_mut().find(|n| n.device_id == id) {
        Some(applied) => *applied = node.clone(),
        None => state.applied.push(node.clone()),
    }
    if let Some(before) = &mut state.before {
        restored_active(&mut before.nodes, node);
    }
    if !state.retained.iter().any(|r| r == id) {
        state.retained.push(id.to_owned());
    }
    true
}
/// `node` is on in the layout `nodes` goes back to.
fn restored_active(nodes: &mut Vec<Node>, node: &Node) {
    match nodes.iter_mut().find(|n| n.device_id == node.device_id) {
        Some(original) => original.active = true,
        None => nodes.push(node.clone()),
    }
}
/// `nodes` with the displays `kept` that are on in `current` on as well: they are held
/// by someone else, and a layout going back must leave them. To its right: where one is
/// now belongs to what is on now: in this layout that can be a gap, or on another display.
fn with_kept(mut nodes: Vec<Node>, current: &[Node], kept: &[String]) -> Vec<Node> {
    for node in current
        .iter()
        .filter(|n| n.active && kept.contains(&n.device_id))
    {
        if !nodes
            .iter()
            .any(|n| n.active && n.device_id == node.device_id)
        {
            nodes = butterpollo_media_policy::display_policy::beside_the_users(&nodes, node);
        }
    }
    nodes
}
/// The layout a first lease goes back to. One an earlier stream left pending comes first,
/// which knows nothing of a remote monitor that arrived since: `retain` has no layout to
/// tell while no lease is held, and a restore would switch off a display still leased.
fn first_baseline(
    pending: Option<Snapshot>,
    original: Option<Snapshot>,
    current: &Snapshot,
    target: &str,
    retained: &[String],
    virtual_target: bool,
) -> Snapshot {
    match pending {
        Some(mut pending) => {
            pending.nodes = with_kept(pending.nodes, &current.nodes, retained);
            pending
        }
        None if virtual_target => {
            original_layout(original.unwrap_or_else(|| current.clone()), target)
        }
        None => current.clone(),
    }
}
fn connected() -> Result<Vec<String>> {
    Ok(Topology::query_all()?
        .monitors()
        .into_iter()
        .map(|m| m.device_id)
        .collect())
}
/// Keep the displays a held layout switched off in the newly composed one. A
/// snapshot lists only the displays that are on, so a layout composed while
/// they are off would forget them, and [`Lease::switched_back_on`] would no
/// longer see Windows switching them on again.
fn keep_switched_off(desired: &mut Vec<Node>, applied: &[Node]) {
    for node in applied.iter().filter(|node| !node.active) {
        if !desired.iter().any(|d| d.device_id == node.device_id) {
            desired.push(node.clone());
        }
    }
}
/// The layout for the first stream's display. Windows can have switched the user's
/// displays off already, recalling the entry an earlier exclusive stream left, and a
/// layout composed from what is on then would not know them ([`keep_switched_off`]).
/// Exclusive only: under the others a display that is off has nothing to come back from.
fn first_layout(
    arrangement: Arrangement,
    current: &[Node],
    target: &str,
    retained: &[String],
    before: &[Node],
    virtual_target: bool,
) -> Result<Vec<Node>> {
    let mut desired = if virtual_target {
        arrangement.compose_after_hotplug(current, target, retained, before)?
    } else {
        arrangement.compose(current, target, retained)?
    };
    if arrangement == Arrangement::Exclusive {
        let off: Vec<Node> = before
            .iter()
            .filter(|n| n.active && !retained.contains(&n.device_id))
            .map(|n| Node {
                active: false,
                primary: false,
                ..n.clone()
            })
            .collect();
        keep_switched_off(&mut desired, &off);
    }
    Ok(desired)
}
/// How many streams' displays have left a layout other streams still hold.
static DEPARTURES: AtomicU64 = AtomicU64::new(0);
/// Compare it to see that a display left while this stream goes on.
pub fn departures() -> u64 {
    DEPARTURES.load(Ordering::Relaxed)
}
/// When a display last left, with the windows minimised just before.
type Departure = (std::time::Instant, std::collections::BTreeSet<isize>);
static LAST_DEPARTURE: Mutex<Option<Departure>> = Mutex::new(None);
/// How long after a display left a window minimised since is put down to it:
/// later, the user is as likely to have done it.
const DEPARTURE_RECENT: std::time::Duration = std::time::Duration::from_secs(10);
/// Note a display leaving. One that follows another soon after keeps the first
/// reading: a window the first one minimised was not the user's doing.
fn note_departure(
    last: &mut Option<Departure>,
    now: std::time::Instant,
    minimized: impl FnOnce() -> std::collections::BTreeSet<isize>,
) {
    match last
        .as_mut()
        .filter(|(at, _)| now.saturating_duration_since(*at) < DEPARTURE_RECENT)
    {
        // The earlier reading stands: what the first display minimised is no user's
        // doing. The streams handle this departure later, so the time moves on.
        Some((at, _)) => *at = now,
        None => *last = Some((now, minimized())),
    }
}
fn recent_departure(
    last: &Option<Departure>,
    now: std::time::Instant,
) -> Option<&std::collections::BTreeSet<isize>> {
    last.as_ref()
        .filter(|(at, _)| now.saturating_duration_since(*at) < DEPARTURE_RECENT)
        .map(|(_, minimized)| minimized)
}
/// The windows that were minimised before a display left, while that departure
/// is recent: any other minimised window is its doing. None after that.
pub fn minimized_before_departure() -> Option<std::collections::BTreeSet<isize>> {
    recent_departure(
        &crate::lock_unpoisoned(&LAST_DEPARTURE),
        std::time::Instant::now(),
    )
    .cloned()
}
/// The streams' displays that are not on (a snapshot lists only the ones that are).
fn targets_off(nodes: &[Node], targets: &[String]) -> Vec<String> {
    targets
        .iter()
        .filter(|t| !nodes.iter().any(|n| n.device_id == **t && n.active))
        .cloned()
        .collect()
}
fn active_ids(nodes: &[Node]) -> std::collections::BTreeSet<String> {
    nodes
        .iter()
        .filter(|n| n.active)
        .map(|n| n.device_id.clone())
        .collect()
}
/// Lay out the current owners' displays and record the result for recovery.
/// `leaving` is switched off unless it was on before streaming started.
fn apply(state: &mut State, arrangement: Arrangement, leaving: Option<&str>) -> Result<()> {
    let mut current = Snapshot::capture()?;
    let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    // Windows can drop a virtual display that has just arrived beside another
    // one while it rearranges them: on again, or there is nothing to lay out.
    let off = targets_off(&current.nodes, &targets);
    if !off.is_empty() {
        tracing::info!(displays = ?off, "switching a stream's display back on before laying it out");
        let mut ids = active_ids(&current.nodes);
        ids.extend(off);
        Topology::set_active(&ids.into_iter().collect::<Vec<_>>())?;
        current = Snapshot::capture()?;
    }
    let physical = physical_displays()?;
    let (mut desired, arriving) =
        compose_around_arrivals(state, arrangement, &current.nodes, &physical)?;
    if let Some(leaving) = leaving {
        let was_active = state.before.as_ref().is_some_and(|before| {
            before
                .nodes
                .iter()
                .any(|n| n.device_id == leaving && n.active)
        });
        if !was_active {
            for node in desired.iter_mut().filter(|n| n.device_id == leaving) {
                node.active = false;
                node.primary = false;
            }
        }
    }
    // A device's two screens come back as the user left them.
    let kept = if leaving.is_none() && remembered::kept_for(&targets, &state.devices, &physical) {
        remembered::apply(&desired, &targets, arrangement.makes_stream_main())
    } else {
        None
    };
    let laid = match kept {
        Some(kept) => lay_out(state, kept, &current.nodes).or_else(|error| {
            // Never the reason a stream fails: the next stream lays them out as usual too.
            tracing::warn!(error = %format!("{error:#}"), displays = ?targets, "Windows refused the kept arrangement of a device's two screens; forgetting it and laying them out as usual");
            remembered::forget(&targets);
            lay_out(state, desired, &current.nodes)
        })?,
        None => lay_out(state, desired, &current.nodes)?,
    };
    settle_layout(state, laid, leaving, &arriving, Topology::save_current);
    Ok(())
}
/// Put `desired` on the displays (on from `current`) and record it for recovery; what it put.
fn lay_out(state: &State, mut desired: Vec<Node>, current: &[Node]) -> Result<Vec<Node>> {
    keep_switched_off(&mut desired, &state.applied);
    if let Some(before) = &state.before {
        crate::display_recovery::arrangement(Some((before.clone(), desired.clone())))?;
    }
    let ids = active_ids(&desired);
    if ids != active_ids(current) {
        Topology::set_active(&ids.into_iter().collect::<Vec<_>>())?;
    }
    Topology::query()?.set_positions(
        &desired
            .iter()
            .filter(|n| n.active)
            .map(|n| (n.device_id.clone(), n.desired_position))
            .collect(),
    )?;
    Ok(desired)
}
/// The connected displays known to be physical (see [`arriving_displays`]).
fn physical_displays() -> Result<Vec<String>> {
    Ok(Topology::query_all()?
        .monitors()
        .into_iter()
        .filter(|m| m.is_known_physical())
        .map(|m| m.device_id)
        .collect())
}
/// The virtual displays on now that the held layout has no place for: one on its way
/// to a stream's lease, a remote monitor's not retained yet. The streams' heartbeats
/// lay out outside the launch that brings them, and a running stream must never lose
/// its display to that. A `physical` display is no such arrival: on with no place in
/// the layout, it is what Windows recalled when a display left (a TV that was off
/// before the stream), and the layout decides about it like any other.
fn arriving_displays(state: &State, current: &[Node], physical: &[String]) -> Vec<String> {
    current
        .iter()
        .filter(|n| {
            n.active
                && !physical.contains(&n.device_id)
                && !state.applied.iter().any(|a| a.device_id == n.device_id)
                && !state.owners.iter().any(|(id, _)| *id == n.device_id)
                && !state.retained.contains(&n.device_id)
        })
        .map(|n| n.device_id.clone())
        .collect()
}
/// The layout for the current owners with what is arriving left on, and what that is.
fn compose_around_arrivals(
    state: &State,
    arrangement: Arrangement,
    current: &[Node],
    physical: &[String],
) -> Result<(Vec<Node>, Vec<String>)> {
    let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    let arriving = arriving_displays(state, current, physical);
    let mut keep = state.retained.clone();
    keep.extend(arriving.iter().cloned());
    let desired = arrangement.compose_all_after_hotplug(
        current,
        &targets,
        &keep,
        state.before.as_ref().map_or(&[], |before| &before.nodes),
    )?;
    Ok((desired, arriving))
}
/// Record the layout just applied, and save it for Windows to recall if it may be.
/// What is `arriving` stays out of the record, as it is still arriving the next time
/// this lays out, and nothing is saved while it does: its own layout does that.
fn settle_layout(
    state: &mut State,
    mut desired: Vec<Node>,
    leaving: Option<&str>,
    arriving: &[String],
    save: impl FnOnce() -> Result<()>,
) {
    desired.retain(|n| !arriving.contains(&n.device_id));
    state.applied = desired;
    if arriving.is_empty()
        && saves_layout(state, leaving)
        && let Err(error) = save()
    {
        tracing::warn!(error = %format!("{error:#}"), "saving the stream layout for Windows to recall failed");
    }
}
/// Whether the layout just applied is saved. A display on its way out is still
/// connected, so saving now would file the layout without it under the entry for
/// both displays, and Windows would recall that the next time they meet: the first
/// stream's display off when the second one arrives. Nothing puts that entry back
/// at the end of the stream. Nor is it saved once no virtual display it persists
/// for is left: the entry would be for the user's displays, or for those and a
/// remote monitor's, with the user's off, and Windows would recall it when the
/// next stream's display goes.
fn saves_layout(state: &State, leaving: Option<&str>) -> bool {
    leaving.is_none() && state.persisted && !state.persists_for.is_empty()
}
/// A new display joins the owners: its entry is saved if it is a virtual display
/// on a layout that saves one.
fn add_owner(state: &mut State, target: &str, virtual_target: bool) {
    state.owners.push((target.to_owned(), 1));
    if state.persisted && virtual_target {
        state.persists_for.push(target.to_owned());
    }
}
/// Notes the device a stream's display streams to. A stream that joins a display a
/// running one owns leaves it that stream's device: the display is still one of that
/// device's two screens, whoever else watches it.
fn note_device(state: &mut State, target: &str, device: &str) {
    if state.users == 0 || !state.owners.iter().any(|(t, _)| t == target) {
        state.devices.insert(target.to_owned(), device.to_owned());
    }
}
/// Undo [`add_owner`]: the layout with that display could not be applied.
fn remove_added_owner(state: &mut State, target: &str) {
    state.owners.retain(|(t, _)| t != target);
    state.persists_for.retain(|id| id != target);
}
/// One lease on `target` ended. `left`: it was the display's last, and the display
/// is no longer an owner. `saved`: the layout was saved for it, so its end has an
/// entry to put back.
#[derive(Debug, PartialEq)]
struct Released {
    left: bool,
    saved: bool,
}
fn release_owner(state: &mut State, target: &str) -> Released {
    let saved = state.persisted && state.persists_for.iter().any(|id| id == target);
    let mut left = false;
    if let Some(index) = state.owners.iter().position(|(t, _)| t == target) {
        state.owners[index].1 -= 1;
        if state.owners[index].1 == 0 {
            state.owners.remove(index);
            state.devices.remove(target);
            state.persists_for.retain(|id| id != target);
            left = true;
        }
    }
    Released { left, saved }
}
/// A virtual display arrived beside the running streams' on a layout that keeps
/// its entry in the Windows display database. Settle that entry now, before
/// anything makes Windows recall it: one saved earlier can switch the running
/// stream's display off, or the new one.
pub fn display_arrived(device_id: &str) -> Result<()> {
    settle_arrival(
        &crate::lock_unpoisoned(state()),
        device_id,
        Topology::set_active,
        Topology::save_current,
    )
}
/// Switch on what is on once `device_id` joined, and save that only as any other
/// layout is ([`saves_layout`]): with no virtual display it persists for left, nothing
/// would ever take that entry back, and Windows would recall the user's monitors off.
fn settle_arrival(
    state: &State,
    device_id: &str,
    mut set_active: impl FnMut(&[String]) -> Result<()>,
    mut save: impl FnMut() -> Result<()>,
) -> Result<()> {
    if state.users == 0 || !state.persisted {
        return Ok(());
    }
    let ids = arrived_layout(&state.owners, &state.retained, device_id);
    settled(|| {
        set_active(&ids)?;
        if saves_layout(state, None) {
            save()
        } else {
            Ok(())
        }
    })
}
/// What is on once `device_id` joined: the streams' displays, what they keep on, and it.
fn arrived_layout(owners: &[(String, usize)], retained: &[String], device_id: &str) -> Vec<String> {
    let mut ids: Vec<String> = owners.iter().map(|(id, _)| id.clone()).collect();
    ids.extend(retained.iter().cloned());
    ids.push(device_id.to_owned());
    ids.sort();
    ids.dedup();
    ids
}
/// How long Windows gets to settle between tries ([`settled`]).
const SETTLE_RETRY: std::time::Duration = std::time::Duration::from_millis(250);
/// Retry while Windows is still applying another change, such as a virtual
/// display being removed: reading its mode fails until the change settles.
fn settled<T>(mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    for _ in 0..3 {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "display layout change pending; retrying");
                std::thread::sleep(SETTLE_RETRY);
            }
        }
    }
    attempt()
}
/// The layout to restore after streaming: Windows extends the desktop onto a
/// new virtual display before the arrangement is taken, but restoring that
/// would leave a retained display as an invisible monitor beside the user's
/// own, and its settings fail once it is removed (os error 31).
fn original_layout(mut snapshot: Snapshot, target: &str) -> Snapshot {
    let others_active = snapshot
        .nodes
        .iter()
        .any(|n| n.active && n.device_id != target);
    if others_active {
        for node in snapshot.nodes.iter_mut().filter(|n| n.device_id == target) {
            node.active = false;
            node.primary = false;
        }
        snapshot.hdr.remove(target);
        snapshot.scale.remove(target);
        snapshot.rotation.remove(target);
    }
    snapshot
}
/// Whether the layout a display left is still as it was applied. The read fails
/// while Windows applies a change, as this departure's own can, and a read that
/// keeps failing says nothing of the user: the streams that go on are laid out.
fn left_as_applied(read: impl FnMut() -> Result<bool>) -> bool {
    settled(read).unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "reading the layout a display left failed; laying out the remaining streams");
        true
    })
}
/// A stream's display left while others go on. A layout still as it was applied
/// is laid out without it, and again once Windows has settled ([`departures`]).
/// Any other layout is the user's (Win+P, a primary picked in Windows), which
/// neither may override: only stop expecting this display.
fn display_left(
    state: &mut State,
    target: &str,
    as_applied: bool,
    apply: impl FnOnce(&mut State) -> Result<()>,
) -> Result<()> {
    if !as_applied {
        for node in state.applied.iter_mut().filter(|n| n.device_id == target) {
            node.active = false;
        }
        return Ok(());
    }
    note_departure(
        &mut crate::lock_unpoisoned(&LAST_DEPARTURE),
        std::time::Instant::now(),
        crate::desktop::minimized_windows,
    );
    DEPARTURES.fetch_add(1, Ordering::Relaxed);
    apply(state)
}

#[cfg(test)]
mod tests;
