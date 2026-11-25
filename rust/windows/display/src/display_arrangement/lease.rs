//! Shared stream arrangement leases, reapplication after hotplug, and teardown.

use super::{
    FollowStep, LeftWatch, Released, State, WakeEnd, active_ids, add_owner, applied_primary_for,
    apply, beside_the_streams, connected, disabled_original_displays, display_left,
    displays_to_watch, first_baseline, first_layout, forget_saved_layout, joins_a_cut_watch,
    lay_out_after_wake, left_as_applied, note_device, persist, physical_displays, recall_to_undo,
    release_owner, remembered, remove_added_owner, repair_saves, settle_layout, settled,
    start_left_watch, start_wake_watch, state, switch_off_strays, unchanged,
    undo_recall_after_departure, wake_follow,
};
use crate::display::{Snapshot, Topology};
use anyhow::Result;
use butterpollo_media_policy::display_policy::Arrangement;
use std::sync::Mutex;

pub struct Lease {
    pub(super) arrangement: Arrangement,
    /// The streamed display; a recreated virtual display may change identity.
    pub(super) target: Mutex<String>,
    /// Why what Windows recalled when this stream's display arrived could not be put back.
    pub(super) repair_failure: Option<String>,
    /// What goes after the lease ([`Lease::end`]).
    pub(super) remains: Mutex<Option<Box<dyn Send>>>,
    /// See [`Lease::after_cut_watch`].
    pub(super) after_cut: bool,
}
/// Log a repair that failed, and return what the stream's card says about it.
pub(super) fn failed_repair(what: &str, error: anyhow::Error) -> String {
    let error = format!("{error:#}");
    tracing::warn!(error = %error, "{what}");
    error
}
/// Repair what Windows recalled, then lay out. A repair that failed doesn't stop the layout;
/// it comes back for the stream's card. A layout that failed stays due for the next try.
pub(super) fn repair_then_lay_out(
    repair: impl FnOnce() -> Result<()>,
    lay_out: impl FnOnce() -> Result<()>,
) -> Result<Option<String>> {
    let failure = repair()
        .err()
        .map(|error| failed_repair("putting back the displays Windows recalled failed", error));
    lay_out()?;
    Ok(failure)
}
impl Lease {
    pub fn exclusive(&self) -> bool {
        self.arrangement == Arrangement::Exclusive
    }
    /// The layout in force for this stream: the running streams', if it joined them.
    pub fn arrangement(&self) -> Arrangement {
        self.arrangement
    }
    /// Whether the layout in force makes this stream's display the primary one. A stream that
    /// joined another's display is laid out beside it, and only the first owner's display is
    /// primary: such a stream has no primary of its own to put back.
    pub fn makes_primary(&self) -> bool {
        let state = crate::lock_unpoisoned(state());
        let target = crate::lock_unpoisoned(&self.target);
        applied_primary_for(&state.applied, target.as_str())
    }
    /// Why what Windows recalled when this stream's display arrived could not be put back.
    pub fn repair_failure(&self) -> Option<&str> {
        self.repair_failure.as_deref()
    }
    /// `virtual_target`: the streamed display is a virtual display made for
    /// streaming, so the user's own layout does not include it. `original`:
    /// the layout taken before that display was created; what Windows
    /// changed when it arrived is then undone with the rest.
    /// `device`: the paired device the stream is for (empty for none), the same for a
    /// device's two screens.
    pub fn acquire(
        output: &str,
        arrangement: Arrangement,
        retained: &[String],
        virtual_target: bool,
        original: Option<Snapshot>,
        device: &str,
    ) -> Result<Self> {
        let monitors = crate::display::basic_monitors()?;
        let target = monitors
            .iter()
            .find(|m| m.matches(output))
            .ok_or_else(|| anyhow::anyhow!("display arrangement target unavailable"))?;
        let mut state = crate::lock_unpoisoned(state());
        let target_id = target.device_id.clone();
        note_device(&mut state, &target_id, device);
        let retained: Vec<_> = monitors
            .iter()
            .filter(|m| retained.contains(&m.display_name) || retained.contains(&m.device_id))
            .map(|m| m.device_id.clone())
            .collect();
        if state.users != 0 {
            // Another stream owns the layout: join it rather than refuse.
            let after_cut = joins_a_cut_watch(state.following.as_ref());
            let owned = state.arrangement.unwrap_or(arrangement);
            if owned != arrangement {
                tracing::info!(requested = ?arrangement, active = ?owned, "joining the display arrangement of the running streams");
            }
            if let Some(owner) = state.owners.iter_mut().find(|(t, _)| *t == target_id) {
                owner.1 += 1;
                state.users += 1;
                return Ok(Self {
                    arrangement: owned,
                    target: Mutex::new(target_id),
                    repair_failure: None,
                    remains: Mutex::default(),
                    after_cut,
                });
            }
            // The displays that are on now can hold an entry a stream on another
            // layout left: the monitor off, or at another mode.
            let mut repair_failure = None;
            if virtual_target
                && let Some(original) = &original
                && recall_to_undo(
                    state.persisted,
                    owned,
                    &original.nodes,
                    &Snapshot::capture()?.nodes,
                    &connected()?,
                )
            {
                tracing::info!(
                    "Windows recalled a saved layout over the other displays for a second stream's display; putting it back"
                );
                repair_failure = forget_saved_layout(original, &target_id, repair_saves(&state))
                    .err()
                    .map(|error| {
                        failed_repair("putting back the saved display layout failed", error)
                    });
            }
            add_owner(&mut state, &target_id, virtual_target);
            let added: Vec<_> = retained
                .into_iter()
                .filter(|id| !state.retained.contains(id))
                .collect();
            state.retained.extend(added.iter().cloned());
            if let Err(error) = apply(&mut state, owned, None) {
                remove_added_owner(&mut state, &target_id);
                state.retained.retain(|id| !added.contains(id));
                return Err(error);
            }
            state.users += 1;
            return Ok(Self {
                arrangement: owned,
                target: Mutex::new(target_id),
                repair_failure,
                remains: Mutex::default(),
                after_cut,
            });
        }
        let mut current = Snapshot::capture()?;
        let pending = crate::display_recovery::pending_arrangement()?;
        let was_pending = pending.is_some();
        let before = first_baseline(
            pending,
            original,
            &current,
            &target_id,
            &retained,
            virtual_target,
        );
        // A host that stopped without putting back the layout it saved for
        // Windows to recall (a crash, a power cut), or a version that put it back
        // wrong, left the user's displays off, at another mode or no longer primary
        // whenever this virtual display arrives. Put it back, unless this stream
        // saves its own.
        let mut repair_failure = None;
        if virtual_target
            && recall_to_undo(
                persist(arrangement, virtual_target),
                arrangement,
                &before.nodes,
                &current.nodes,
                &connected()?,
            )
        {
            tracing::info!(
                "Windows recalled a saved layout over the user's displays for the virtual display; putting it back"
            );
            match forget_saved_layout(&before, &target_id, true).and_then(|()| Snapshot::capture())
            {
                Ok(recaptured) => current = recaptured,
                Err(error) => {
                    repair_failure = Some(failed_repair(
                        "putting back the saved display layout failed",
                        error,
                    ));
                }
            }
        }
        let following = if was_pending {
            wake_follow(
                arrangement,
                &before.nodes,
                &connected()?,
                &target_id,
                &retained,
            )
        } else {
            None
        };
        if following.is_some() {
            // What Windows lit while the woken display is away, before anything is composed
            // around it. An exclusive layout takes it off by itself.
            let mut keep = retained.clone();
            keep.push(target_id.clone());
            match switch_off_strays(&before.nodes, &keep, true).and_then(|()| Snapshot::capture()) {
                Ok(recaptured) => current = recaptured,
                Err(error) => {
                    repair_failure = Some(failed_repair(
                        "switching off a display Windows recalled while the user's monitor is away failed",
                        error,
                    ));
                }
            }
        }
        let desired = first_layout(
            arrangement,
            &current.nodes,
            &target_id,
            &retained,
            &before.nodes,
            virtual_target,
        )?;
        crate::display_recovery::arrangement(Some((before.clone(), desired.clone())))?;
        let apply = (|| -> Result<()> {
            let ids = active_ids(&desired);
            if ids != active_ids(&current.nodes) {
                Topology::set_active(&ids.into_iter().collect::<Vec<_>>())?;
            }
            Topology::query()?.set_positions(
                &desired
                    .iter()
                    .filter(|n| n.active)
                    .map(|n| (n.device_id.clone(), n.desired_position))
                    .collect(),
            )
        })();
        if let Err(error) = apply {
            // A pending recovery may have supplied an older, healthy baseline.
            // Only restoring that baseline permits clearing its journal, and with a
            // display of it away that isn't done: see `left_watch`.
            before.restore()?;
            if !was_pending || before.displays_connected() {
                crate::display_recovery::arrangement(None)?;
            }
            return Err(error);
        }
        let after_cut = following.is_some();
        state.following = following;
        state.before = Some(before);
        state.users = 1;
        state.key = format!("{arrangement:?}:{target_id}");
        state.owners.clear();
        state.devices.retain(|t, _| *t == target_id);
        state.persists_for.clear();
        state.persisted = persist(arrangement, virtual_target);
        add_owner(&mut state, &target_id, virtual_target);
        state.arrangement = Some(arrangement);
        state.retained = retained;
        settle_layout(&mut state, desired, None, &[], Topology::save_current);
        Ok(Self {
            arrangement,
            target: Mutex::new(target_id),
            repair_failure,
            remains: Mutex::default(),
            after_cut,
        })
    }
    /// End the lease, and with it what `remains` of the stream: its display above all,
    /// which outlasts the lease only as long as the watch over a woken display needs it.
    /// That takes no one's time: this returns once the layout went back.
    pub fn end(self, remains: impl Send + 'static) {
        *crate::lock_unpoisoned(&self.remains) = Some(Box::new(remains));
    }
    /// One look on the heartbeat at a display a stopped watch was waiting for, on a layout
    /// that took that up ([`WakeFollow`]). True when its return took a layout pass that can
    /// have recalled the stream's display's mode. An error is the last try's.
    pub fn follow_wake(&self) -> Result<bool> {
        let mut guard = crate::lock_unpoisoned(state());
        let state = &mut *guard;
        let Some(follow) = state.following.as_mut().filter(|f| !f.done) else {
            return Ok(false);
        };
        let listed = connected().ok();
        let step = follow.step(follow.started.elapsed(), listed.as_deref());
        match step {
            FollowStep::Nothing => Ok(false),
            FollowStep::SwitchOffStrays => {
                let before = state.before.as_ref().map_or(&[][..], |b| &b.nodes);
                let switched = Snapshot::capture().and_then(|current| {
                    let (keep, arrives) = beside_the_streams(
                        state,
                        &current.nodes,
                        &physical_displays()?,
                        &crate::display::leased_displays(),
                    );
                    switch_off_strays(before, &keep, !arrives)
                });
                let follow = state.following.as_mut().expect("checked above");
                if let Err(error) = switched
                    && !std::mem::replace(&mut follow.failed_strays, true)
                {
                    tracing::warn!(error = %format!("{error:#}"), "switching off a display Windows recalled while the user's monitor is away failed");
                }
                Ok(false)
            }
            FollowStep::LayOut => {
                let arrangement = state.arrangement.unwrap_or(self.arrangement);
                let result = lay_out_after_wake(state, arrangement);
                let follow = state.following.as_mut().expect("checked above");
                match result {
                    Ok((put_back, failure)) => {
                        follow.tried(true);
                        match failure {
                            Some(failure) => Err(anyhow::anyhow!(failure)),
                            None => Ok(put_back),
                        }
                    }
                    Err(error) if follow.tried(false) => {
                        tracing::debug!(error = %format!("{error:#}"), "display layout change pending; retrying");
                        Ok(false)
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }
    /// This stream took up what a stopped watch left, or joined while that was at work: what
    /// is on can have been up to Windows at any time in it, a monitor away for one. Nobody
    /// chose that. One joining later has a layout like any other.
    pub fn after_cut_watch(&self) -> bool {
        self.after_cut
    }
    /// Displays this stream's layout switched off that Windows has switched
    /// back on, such as when an exclusive-fullscreen game loses focus and
    /// Windows recalls its saved layout for the connected displays.
    pub fn switched_back_on(&self) -> Result<Vec<String>> {
        // Four times a second for each stream: the lean read, not `monitors`.
        let active = crate::display::active_device_ids()?;
        let state = crate::lock_unpoisoned(state());
        let Some(before) = &state.before else {
            return Ok(Vec::new());
        };
        Ok(butterpollo_media_policy::display_policy::switched_back_on(
            &before.nodes,
            &state.applied,
            &active,
        ))
    }
    /// Reapply the stream layout after an owned VDD is recreated, retaining the
    /// original restoration snapshot and recovery journal.
    pub fn reapply(&self, output: &str, retained: &[String]) -> Result<()> {
        let monitors = crate::display::basic_monitors()?;
        let mut state = crate::lock_unpoisoned(state());
        let mut owned = crate::lock_unpoisoned(&self.target);
        let target = reapplied_target(&monitors, output, &owned)
            .ok_or_else(|| anyhow::anyhow!("recovered arrangement target unavailable"))?;
        if *owned != target.device_id {
            for owner in state.owners.iter_mut().filter(|(t, _)| *t == *owned) {
                owner.0 = target.device_id.clone();
            }
            if let Some(device) = state.devices.remove(&*owned) {
                state.devices.insert(target.device_id.clone(), device);
            }
            for id in state.persists_for.iter_mut().filter(|id| **id == *owned) {
                *id = target.device_id.clone();
            }
            *owned = target.device_id.clone();
        }
        for id in monitors
            .iter()
            .filter(|m| retained.contains(&m.display_name) || retained.contains(&m.device_id))
        {
            if !state.retained.contains(&id.device_id) {
                state.retained.push(id.device_id.clone());
            }
        }
        let arrangement = state.arrangement.unwrap_or(self.arrangement);
        apply(&mut state, arrangement, None)
    }
    /// [`Self::reapply`] once another stream's display has left and Windows has
    /// settled ([`departures`]). What it recalled for the displays that remain is
    /// undone first ([`departure_repair`]). That can also undo the user's change in
    /// the same seconds, which is then not saved; a change before the display left
    /// counts no departure.
    pub fn reapply_after_departure(
        &self,
        output: &str,
        retained: &[String],
    ) -> Result<Option<String>> {
        repair_then_lay_out(
            || {
                let state = crate::lock_unpoisoned(state());
                let arrangement = state.arrangement.unwrap_or(self.arrangement);
                undo_recall_after_departure(&state, arrangement)
            },
            || self.reapply(output, retained),
        )
    }
}
/// The last lease on a layout ended: put back the entry it `saved` and the user's layout.
/// What to watch afterwards, in which case the journal entry stays for the watch to clear.
fn end_layout(state: &State, target: &str, saved: bool) -> Result<Option<WakeEnd>> {
    // Put the saved entry back even when the user changed the layout:
    // Windows recalls the entry for the remaining displays anyway once
    // the virtual display is removed.
    let restore = state
        .before
        .as_ref()
        .is_some_and(|before| disabled_original_displays(&before.nodes, &state.applied))
        || settled(|| unchanged(&state.applied))?;
    let Some(before) = state.before.as_ref() else {
        crate::display_recovery::arrangement(None)?;
        return Ok(None);
    };
    let woken = if saved {
        displays_to_watch(before, target)
    } else {
        Vec::new()
    };
    // Issue #129: log each layout commit of the way back and the windows that moved.
    let mut trace = (saved || restore).then(crate::display::end_trace::Trace::start);
    let mut outcome = |step, ok| {
        if let Some(trace) = trace.as_mut() {
            trace.outcome(step, ok);
        }
    };
    if !woken.is_empty() {
        // One try each: a display dropping now fails them, and the watch tries again.
        let first = forget_saved_layout(before, target, true);
        outcome("entry", first.is_ok());
        let restored = if restore {
            crate::display::end_trace::phase("user's layout");
            let restored = before.restore();
            outcome("user's layout", restored.is_ok());
            restored
        } else {
            Ok(())
        };
        drop(trace);
        return Ok(Some(WakeEnd {
            woken,
            restore,
            first,
            restored,
        }));
    }
    if saved {
        let entry = settled(|| forget_saved_layout(before, target, true));
        outcome("entry", entry.is_ok());
        if let Err(error) = entry {
            tracing::warn!(error = %format!("{error:#}"), "putting back the saved display layout failed");
        }
    }
    let restored = if restore {
        crate::display::end_trace::phase("user's layout");
        let restored = settled(|| before.restore());
        outcome("user's layout", restored.is_ok());
        restored
    } else {
        Ok(())
    };
    // Logs what the windows did, before a failure returns.
    drop(trace);
    restored?;
    crate::display_recovery::arrangement(None)?;
    Ok(None)
}
impl Drop for Lease {
    fn drop(&mut self) {
        // Declared before the lock, so dropped after it: removing a display takes a while.
        let mut remains = crate::lock_unpoisoned(&self.remains).take();
        let mut state = crate::lock_unpoisoned(state());
        let target = crate::lock_unpoisoned(&self.target).clone();
        state.users -= 1;
        // One of a device's two screens leaves: keep how they are arranged now. Read
        // under the lock like the rest of this layout work (`unchanged` and `apply` below), so
        // no other stream's layout lands between the read and the owners it is kept for.
        let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
        if targets.len() == 2
            && state.owners.iter().any(|(t, n)| *t == target && *n == 1)
            && let Ok(physical) = physical_displays()
            && remembered::kept_for(&targets, &state.devices, &physical)
            && let Ok(topology) = Topology::query().and_then(|t| t.nodes())
        {
            remembered::remember(&topology, &targets);
        }
        let Released { left, saved } = release_owner(&mut state, &target);
        if state.users != 0 {
            // Other streams continue: lay out their displays without this one.
            if left {
                let arrangement = state.arrangement.unwrap_or(self.arrangement);
                let as_applied = left_as_applied(|| unchanged(&state.applied));
                let result = display_left(&mut state, &target, as_applied, |state| {
                    apply(state, arrangement, Some(&target))
                });
                if let Err(error) = result {
                    tracing::warn!(%error, "display arrangement update for the remaining streams failed");
                }
            }
            return;
        }
        let ended = end_layout(&state, &target, saved);
        let before = state.before.take();
        state.persisted = false;
        state.persists_for.clear();
        state.applied.clear();
        state.key.clear();
        state.owners.clear();
        state.devices.clear();
        state.arrangement = None;
        state.retained.clear();
        state.following = None;
        drop(state);
        // Issue #129: the windows again once apps and Windows settled, on its own thread,
        // after the stream's display is gone: `remains` goes with this lease, or with the
        // watch that keeps it.
        let baseline = crate::display::end_trace::take_baseline();
        match what_an_end_leaves(ended, before, &target, &crate::display::leased_displays()) {
            EndLeaves::Nothing => crate::display::end_trace::settle_later(baseline),
            EndLeaves::Watch(before, end) => {
                start_wake_watch(before, target, end, remains.take(), baseline);
            }
            EndLeaves::Left(left) => {
                start_left_watch(left);
                crate::display::end_trace::settle_later(baseline);
            }
        }
    }
}
/// What the last lease's end leaves to a thread.
pub(super) enum EndLeaves {
    Nothing,
    Watch(Snapshot, WakeEnd),
    Left(LeftWatch),
}
/// An end that failed goes to a watch as a cut one does ([`left_watch`]). What fails is a
/// display of the layout dropping as this switched it on: the others alone with nothing at
/// the origin, Windows answers invalid parameter until it returns. The journal entry must
/// not sit until the next stream takes it for a baseline, so the layout goes back then.
pub(super) fn what_an_end_leaves(
    ended: Result<Option<WakeEnd>>,
    before: Option<Snapshot>,
    target: &str,
    held: &[String],
) -> EndLeaves {
    match (ended, before) {
        (Ok(Some(end)), Some(before)) => EndLeaves::Watch(before, end),
        (Ok(_), _) => EndLeaves::Nothing,
        (Err(error), Some(before)) => {
            tracing::warn!(error = %format!("{error:#}"), "the user's layout did not go back at a stream's end; trying again once its displays stayed or returned");
            EndLeaves::Left(LeftWatch {
                woken: before
                    .nodes
                    .iter()
                    .filter(|n| n.active && n.device_id != target && !held.contains(&n.device_id))
                    .map(|n| n.device_id.clone())
                    .collect(),
                restore: true,
            })
        }
        (Err(error), None) => {
            tracing::warn!(error = %format!("{error:#}"), "display arrangement restoration remains pending");
            EndLeaves::Nothing
        }
    }
}
/// The stream's display among the active monitors: by the output name the
/// stream knows, else by the device id the lease holds. Windows can list a
/// display under another desktop name (issue 56) before the stream learns it.
pub(super) fn reapplied_target<'a>(
    monitors: &'a [crate::display::Monitor],
    output: &str,
    owned: &str,
) -> Option<&'a crate::display::Monitor> {
    monitors.iter().find(|m| m.matches(output)).or_else(|| {
        monitors
            .iter()
            .find(|m| !owned.is_empty() && m.device_id == owned)
    })
}
