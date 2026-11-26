//! Windows display database entry persistence and recall repair.

use super::{
    State, active_ids, arriving_displays, connected, disabled_original_displays, physical_displays,
    saves_layout, state, wake_watch_settling,
};
use crate::display::{Snapshot, Topology};
use anyhow::Result;
use butterpollo_media_policy::{display_policy::Arrangement, topology::Node};

/// Whether to save a stream's layout to the Windows display database. Windows
/// recalls the saved layout for the connected displays when an
/// exclusive-fullscreen game loses focus (the Win key, Alt+Tab), which would
/// switch the physical monitor back on. Only a layout with the stream's own
/// virtual display: the database entry is keyed by the connected displays, so
/// this changes only the entry for the user's displays plus that virtual
/// display, never the one for the user's displays alone. The entry is put
/// back when the last stream ends (`forget_saved_layout`).
pub(super) fn persist(arrangement: Arrangement, virtual_target: bool) -> bool {
    virtual_target && arrangement == Arrangement::Exclusive
}
/// Put back the database entry a stream saved: the user's displays as they have
/// them with the virtual display beside them, as Windows lays out a new display.
/// Called before the user's layout is restored, while the virtual display is
/// still connected. Switching them on is not enough: the virtual display would
/// stay primary and a monitor Windows switches on gets Windows' choice of timing
/// (60 Hz on a 120 Hz monitor), and every later stream that keeps it on is handed
/// that entry.
pub(super) fn forget_saved_layout(before: &Snapshot, target: &str, saves: bool) -> Result<()> {
    let primary = before
        .nodes
        .iter()
        .find(|n| n.active && n.primary)
        .map(|n| n.device_id.as_str());
    put_entry_back(
        before,
        target,
        connected,
        |ids| {
            crate::display::end_trace::phase("entry, displays on");
            Topology::set_active_preferred(ids, &before.sources, &before.rotation, primary)
        },
        || Ok(Snapshot::capture()?.nodes),
        |layout| {
            crate::display::end_trace::phase("entry layout");
            layout.restore()
        },
        || {
            if saves {
                Topology::save_current()
            } else {
                Ok(())
            }
        },
    )
}
/// Whether a repair while streams hold a layout may save what it put back. The entry
/// is for the displays connected now. Saved while the held layout keeps a display of
/// the user's off, it has that display off, and unless the layout saves its own
/// ([`saves_layout`]) nothing takes that back: Windows recalls it when a later
/// virtual display leaves, and the monitor stays dark. An isolated layout saves
/// nothing either: its displays are 64000 apart on purpose, which is no entry for
/// Windows to hand a later stream.
pub(super) fn repair_saves(state: &State) -> bool {
    if saves_layout(state, None) {
        return true;
    }
    !isolated(state)
        && !state
            .before
            .as_ref()
            .is_some_and(|before| disabled_original_displays(&before.nodes, &state.applied))
}
pub(super) fn isolated(state: &State) -> bool {
    matches!(
        state.arrangement,
        Some(Arrangement::Isolated | Arrangement::PrimaryIsolated)
    )
}
/// The steps of [`forget_saved_layout`]. `set_active` drops a display with no path
/// left (a driver reset, a lost device) and switches on the rest at Windows' timing.
/// With the stream's display gone, what is connected is the user's displays alone:
/// saved now, that timing is what Windows recalls when a later virtual display
/// leaves, and it then reads as the user's own. So nothing is saved unless that
/// display is on. What is `connected` is read before anything switches on: a monitor
/// coming up can drop off that list for a moment, and left out of the layout for it
/// the layout would switch it off again.
pub(super) fn put_entry_back(
    before: &Snapshot,
    target: &str,
    connected: impl FnOnce() -> Result<Vec<String>>,
    set_active: impl FnOnce(&[String]) -> Result<()>,
    read: impl FnOnce() -> Result<Vec<Node>>,
    restore: impl FnOnce(&Snapshot) -> Result<()>,
    save: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let connected = connected()?;
    let mut ids: Vec<_> = before
        .nodes
        .iter()
        .filter(|n| n.active && n.device_id != target)
        .map(|n| n.device_id.clone())
        .collect();
    ids.push(target.to_owned());
    set_active(&ids)?;
    let current = read()?;
    let layout = entry_layout(before, &current, &connected, target)?;
    let laid_out = restore(&layout);
    // A layout that only partly went back still beats the entry with them off,
    // and this entry is the stream's display's own.
    save()?;
    laid_out
}
/// `before` with the virtual display `target`, as it is in `current`, beside it. A
/// display no longer `connected` has no place in it: `before` keeps a departed remote
/// monitor's node, and the entry would have a gap where it was.
pub(super) fn entry_layout(
    before: &Snapshot,
    current: &[Node],
    connected: &[String],
    target: &str,
) -> Result<Snapshot> {
    let target = current
        .iter()
        .find(|n| n.device_id == target && n.active)
        .ok_or_else(|| anyhow::anyhow!("the stream's display is off"))?;
    let present: Vec<_> = before
        .nodes
        .iter()
        .filter(|n| connected.contains(&n.device_id))
        .cloned()
        .collect();
    let mut layout = before.clone();
    layout.nodes = butterpollo_media_policy::display_policy::beside_the_users(&present, target);
    Ok(layout)
}
/// Whether a virtual display's arrival handed a layout that keeps the other
/// displays on an entry from the Windows display database that does not: it can
/// come from an exclusive stream (a crash left its own in place, a second stream's
/// entry has nothing to put it back) or from a version that saved it wrong. One
/// that `saves` its own has no use for the entry.
pub(super) fn recall_to_undo(
    saves: bool,
    arrangement: Arrangement,
    before: &[Node],
    current: &[Node],
    connected: &[String],
) -> bool {
    !saves
        && butterpollo_media_policy::display_policy::recalled_over(
            before,
            current,
            connected,
            &[],
            arrangement.keeps_primary(),
        )
}
/// A remote monitor's display `output` has arrived, `before` being the layout from
/// just before it. On a layout that leaves the others alone it holds no lease to
/// look after them, so what Windows recalled for it is undone here. A layout that
/// does move the primary or switches them off applies that afterwards.
pub fn undo_recall_for_remote_monitor(before: &Snapshot, output: &str) -> Result<()> {
    // Before the lock: stopping a watch holds the watcher across a wait for a thread that
    // wants this one.
    let settling = wake_watch_settling();
    let state = crate::lock_unpoisoned(state());
    let pending = if state.users == 0 && settling {
        crate::display_recovery::pending_arrangement()?
    } else {
        None
    };
    let before = remote_monitor_baseline(state.users, settling, pending.as_ref(), before);
    let Some(saves) = remote_monitor_repair(
        &state,
        &before.nodes,
        &Snapshot::capture()?.nodes,
        &connected()?,
    ) else {
        return Ok(());
    };
    let target = crate::display::monitors()?
        .into_iter()
        .find(|m| m.matches(output))
        .ok_or_else(|| anyhow::anyhow!("the remote monitor's display is gone"))?
        .device_id;
    tracing::info!(
        "Windows recalled a saved layout over the other displays for a remote monitor's display; putting it back"
    );
    forget_saved_layout(before, &target, saves)
}
/// The layout a remote monitor's arrival goes by: the one from just before it, unless a
/// stopped watch left the user's: with their monitor away what was on then can be what
/// Windows lit, and an entry saved from that would light it for later streams. Only while
/// that is at work (`settling`): an entry stuck since then knows nothing of what the user
/// changed after it, and this repair saves.
pub(super) fn remote_monitor_baseline<'a>(
    users: usize,
    settling: bool,
    pending: Option<&'a Snapshot>,
    before: &'a Snapshot,
) -> &'a Snapshot {
    pending.filter(|_| users == 0 && settling).unwrap_or(before)
}
/// Whether a remote monitor's arrival needs a repair, and if so whether it saves.
/// It is judged by the layout the streams hold; with none held, by one that leaves
/// the others alone, as its own applies afterwards.
pub(super) fn remote_monitor_repair(
    state: &State,
    before: &[Node],
    current: &[Node],
    connected: &[String],
) -> Option<bool> {
    let held = state.arrangement.filter(|_| state.users != 0);
    recall_to_undo(
        false,
        held.unwrap_or(Arrangement::Extended),
        before,
        current,
        connected,
    )
    .then(|| repair_saves(state))
}
/// The layout that puts back what Windows recalled once another stream's display
/// has gone: `applied`, the layout from when it left, with what else is on as it is.
pub(super) fn after_departure(applied: &[Node], before: Option<&Snapshot>) -> Snapshot {
    Snapshot {
        version: 1,
        nodes: applied.to_vec(),
        hdr: Default::default(),
        clone_groups: before.map_or_else(Vec::new, |before| before.clone_groups.clone()),
        scale: Default::default(),
        rotation: Default::default(),
        sources: before.map_or_else(Default::default, |before| before.sources.clone()),
    }
}
/// What putting back the layout a display left takes.
#[derive(Debug, PartialEq)]
pub(super) struct DepartureRepair {
    /// See [`arriving_displays`]: the repair leaves these on.
    pub(super) keep_on: Vec<String>,
    /// What the layout has on, of what is connected.
    pub(super) on: std::collections::BTreeSet<String>,
    /// Whether to save it. Not when what is on differs from that layout: this can't
    /// tell Windows switching a monitor off from the user's Win+P in the same seconds,
    /// and what is arriving has its entry saved with its own layout. The first stream
    /// on such an entry repairs it.
    pub(super) saves: bool,
}
/// Whether what is on now is what Windows recalled over the layout a display left, and
/// what putting that back takes. None on a layout that saves its own entry.
pub(super) fn departure_repair(
    state: &State,
    arrangement: Arrangement,
    current: &[Node],
    connected: &[String],
    physical: &[String],
) -> Option<DepartureRepair> {
    if state.persisted {
        return None;
    }
    let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    if !butterpollo_media_policy::display_policy::recalled_over(
        &state.applied,
        current,
        connected,
        &targets,
        arrangement.keeps_primary(),
    ) {
        return None;
    }
    let on: std::collections::BTreeSet<String> = state
        .applied
        .iter()
        .filter(|n| n.active && connected.contains(&n.device_id))
        .map(|n| n.device_id.clone())
        .collect();
    Some(DepartureRepair {
        keep_on: arriving_displays(state, current, physical),
        saves: repair_saves(state) && active_ids(current) == on,
        on,
    })
}
/// Another stream's display has gone and Windows has recalled its entry for the
/// displays that remain. On a layout that keeps the others on, that entry can have
/// one of them off or at another mode: put it back, and save it so that it stays.
pub(super) fn undo_recall_after_departure(state: &State, arrangement: Arrangement) -> Result<()> {
    let current = Snapshot::capture()?.nodes;
    let Some(repair) = departure_repair(
        state,
        arrangement,
        &current,
        &connected()?,
        &physical_displays()?,
    ) else {
        return Ok(());
    };
    tracing::info!(
        keep_on = ?repair.keep_on,
        saves = repair.saves,
        "Windows recalled a saved layout over the other displays when a stream's display left; putting it back"
    );
    after_departure(&state.applied, state.before.as_ref()).restore_excluding(&repair.keep_on)?;
    // Not if anything else changed while it went back.
    if repair.saves && active_ids(&Snapshot::capture()?.nodes) == repair.on {
        Topology::save_current()?;
    }
    Ok(())
}
