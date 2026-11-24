//! In-stream follow-up after a launch cuts a background wake watch short.

use super::{
    State, Wake, WakeLook, WakeLooks, active_ids, all_listed, apply, arriving_displays, connected,
    failed_repair, forget_saved_layout, isolated, physical_displays, strays, with_kept,
};
use crate::display::Snapshot;
use anyhow::Result;
use butterpollo_media_policy::{display_policy::Arrangement, topology::Node};
use std::sync::atomic::{AtomicU64, Ordering};

/// How many times a monitor's return had displays put back ([`lay_out_after_wake`]).
static WAKE_LAYOUTS: AtomicU64 = AtomicU64::new(0);
/// Compare it to see that this happened while this stream goes on: one heartbeat does it,
/// and it can have recalled the mode of any stream's display.
pub fn wake_layouts() -> u64 {
    WAKE_LAYOUTS.load(Ordering::Relaxed)
}
/// How often a stream that took up a stopped watch's layout tries to lay it out once the
/// woken display showed, one heartbeat apart ([`Lease::follow_wake`]).
pub(super) const FOLLOW_TRIES: usize = 8;
/// What a non-exclusive stream does for what a stopped watch left. It took the layout over
/// with the woken display perhaps still to drop, or away: then no layout it composes knows
/// that display, what Windows lit in its place stays on, and the display may stay dark. So
/// its heartbeat looks on, as the watch did ([`Lease::follow_wake`]). Under an exclusive
/// layout there is nothing to do: everything else is off, and comes back as any recall.
#[derive(Debug)]
pub(super) struct WakeFollow {
    pub(super) woken: Vec<String>,
    pub(super) started: std::time::Instant,
    pub(super) looks: WakeLooks,
    pub(super) tries: usize,
    pub(super) repair: WakeRepair,
    pub(super) done: bool,
    pub(super) failed_strays: bool,
}
/// How the repair of a returned display's layout went, across the layout's tries
/// ([`pass_after_wake`]).
#[derive(Debug, Default)]
pub(super) struct WakeRepair {
    /// The displays have been put back: no second pass on a running stream.
    pub(super) done: bool,
    /// What a display refused, until the layout gets through and the card can say it.
    pub(super) refused: Option<String>,
}
/// Whether a stream joining a layout came while a stopped watch's display is still looked
/// for. Once that is done, what is on is as anyone's.
pub(super) fn joins_a_cut_watch(following: Option<&WakeFollow>) -> bool {
    following.is_some_and(|f| !f.done)
}
/// Follow up on a pending layout: the displays of `before` others don't hold, some of them
/// away (not `connected`) already, as a lease starts.
pub(super) fn wake_follow(
    arrangement: Arrangement,
    before: &[Node],
    connected: &[String],
    target: &str,
    retained: &[String],
) -> Option<WakeFollow> {
    if arrangement == Arrangement::Exclusive {
        return None;
    }
    let woken: Vec<String> = before
        .iter()
        .filter(|n| n.active && n.device_id != target && !retained.contains(&n.device_id))
        .map(|n| n.device_id.clone())
        .collect();
    if woken.is_empty() {
        return None;
    }
    let looks = WakeLooks {
        gone_since: (!all_listed(&woken, connected)).then_some(std::time::Duration::ZERO),
    };
    Some(WakeFollow {
        woken,
        started: std::time::Instant::now(),
        looks,
        tries: 0,
        repair: WakeRepair::default(),
        done: false,
        failed_strays: false,
    })
}
/// What a look at the woken display calls for.
#[derive(Debug, PartialEq)]
pub(super) enum FollowStep {
    Nothing,
    SwitchOffStrays,
    LayOut,
}
impl WakeFollow {
    /// `now`: how long the stream's layout has been there.
    pub(super) fn step(
        &mut self,
        now: std::time::Duration,
        connected: Option<&[String]>,
    ) -> FollowStep {
        if self.done {
            return FollowStep::Nothing;
        }
        let present = connected.map(|c| all_listed(&self.woken, c));
        match self.looks.look(now, present) {
            WakeLook::Watching => FollowStep::Nothing,
            WakeLook::Away => FollowStep::SwitchOffStrays,
            WakeLook::Done(Wake::StayedAway) => {
                tracing::warn!(displays = ?self.woken, "a display dropped off the list when it woke and did not come back in time");
                self.done = true;
                FollowStep::Nothing
            }
            WakeLook::Done(_) => FollowStep::LayOut,
        }
    }
    /// One try at the layout has gone. True while there is another.
    pub(super) fn tried(&mut self, succeeded: bool) -> bool {
        self.tries += 1;
        self.done = succeeded || self.tries >= FOLLOW_TRIES;
        !self.done
    }
}
/// Whether the layout needs its displays put back before it is laid out: the woken display
/// off, at another mode or (`keeps_primary`) no longer primary, or Windows lit something
/// else. A monitor that came back as it was only has its place to get.
pub(super) fn wake_layout_wrong(
    before: &[Node],
    current: &[Node],
    connected: &[String],
    physical: &[String],
    streams: &[String],
    keep: &[String],
    keeps_primary: bool,
) -> bool {
    let active: Vec<_> = active_ids(current).into_iter().collect();
    butterpollo_media_policy::display_policy::recalled_over(
        before,
        current,
        connected,
        streams,
        keeps_primary,
    ) || !strays(before, &active, physical, keep).0.is_empty()
}
/// What a pass for a returned display leaves on beside the streams: what they and launches
/// and remote monitors (`leased`) hold, and what is on its way to one. And whether anything
/// is: then what is on is no entry to save ([`DepartureRepair::saves`]).
pub(super) fn beside_the_streams(
    state: &State,
    current: &[Node],
    physical: &[String],
    leased: &[String],
) -> (Vec<String>, bool) {
    let arriving = arriving_displays(state, current, physical);
    let arrives = !arriving.is_empty();
    let mut keep: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    keep.extend(state.retained.iter().cloned());
    keep.extend(leased.iter().cloned());
    keep.extend(arriving);
    (keep, arrives)
}
/// The user's layout around what is kept on, of which only the place is its business. HDR,
/// scale, rotation and mode there are what the streams set, not to be undone mid-stream.
/// Not excluded from the restore: left where it is, it can be where the user's monitor goes.
pub(super) fn layout_around_streams(
    before: Snapshot,
    current: &[Node],
    keep: &[String],
) -> Snapshot {
    let mut layout = Snapshot {
        nodes: with_kept(before.nodes, current, keep),
        ..before
    };
    for id in keep {
        layout.hdr.remove(id);
        layout.scale.remove(id);
        layout.rotation.remove(id);
        let now = current.iter().find(|n| n.active && n.device_id == *id);
        let node = layout.nodes.iter_mut().find(|n| n.device_id == *id);
        if let (Some(node), Some(now)) = (node, now) {
            node.mode = now.mode.clone();
        }
    }
    layout
}
/// One go at a returned display's layout. A repair that got through runs once however many
/// goes the layout takes, and so does one a display refused (its saved mode or HDR): that
/// monitor gets no more passes on a running stream, and the stream gets its layout anyway.
/// Any other failed repair is an error, tried again as a layout is: a change pending just
/// as the monitor came back must not leave it dark for the rest of the stream.
///
/// A layout laid out after a repair is what can have recalled a stream's mode, whichever
/// go it took, so that is when `layouts`, which the streams read, moves. What comes back is
/// what a display refused, for the card.
pub(super) fn pass_after_wake(
    state: &mut WakeRepair,
    wrong: bool,
    layouts: &AtomicU64,
    repair: impl FnOnce() -> Result<()>,
    lay_out: impl FnOnce() -> Result<()>,
) -> Result<Option<String>> {
    if wrong && !state.done {
        match repair() {
            Ok(()) => state.done = true,
            Err(error) if crate::display::restore_was_refused(&error) => {
                state.done = true;
                state.refused = Some(failed_repair(
                    "putting back the displays Windows recalled failed",
                    error,
                ));
            }
            Err(error) => return Err(error),
        }
    }
    lay_out()?;
    if state.done {
        layouts.fetch_add(1, Ordering::Relaxed);
    }
    Ok(state.refused.take())
}
/// The woken display stayed or returned: the layout the stream planned, with it at its
/// mode. What is right already isn't written, so this is one layout pass at most. Whether
/// the displays were put back, and what came of that if it failed.
pub(super) fn lay_out_after_wake(
    state: &mut State,
    arrangement: Arrangement,
) -> Result<(bool, Option<String>)> {
    let Some(before) = state.before.clone() else {
        return Ok((false, None));
    };
    let targets: Vec<_> = state.owners.iter().map(|(t, _)| t.clone()).collect();
    let Some(target) = targets.first() else {
        return Ok((false, None));
    };
    let current = Snapshot::capture()?.nodes;
    let physical = physical_displays()?;
    let (keep, arrives) = beside_the_streams(
        state,
        &current,
        &physical,
        &crate::display::leased_displays(),
    );
    let wrong = wake_layout_wrong(
        &before.nodes,
        &current,
        &connected()?,
        &physical,
        &targets,
        &keep,
        arrangement.keeps_primary(),
    );
    // Saved as any repair is but for the woken display, which the layout never had off.
    let saves = !isolated(state) && !arrives;
    let mut repair = state
        .following
        .as_mut()
        .map(|f| std::mem::take(&mut f.repair))
        .unwrap_or_default();
    let pass = pass_after_wake(
        &mut repair,
        wrong,
        &WAKE_LAYOUTS,
        || {
            tracing::info!(
                "a display a stopped watch was waiting for is back; putting back the layout around the stream"
            );
            let layout = layout_around_streams(before, &current, &keep);
            forget_saved_layout(&layout, target, saves)
        },
        || apply(state, arrangement, None),
    );
    let put_back = repair.done;
    if let Some(follow) = state.following.as_mut() {
        follow.repair = repair;
    }
    Ok((put_back, pass?))
}
