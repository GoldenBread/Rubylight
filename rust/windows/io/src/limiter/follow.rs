//! Dynamic RTSS frame-limit stand-down when a virtual display's refresh drops below the cap.
use super::{State, confirmed, load, persist, state, warn_unsupported};
use crate::rtss;
use anyhow::{Result, bail};
use butterpollo_media_policy::framegen::Rate;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// A virtual display's automatic cap, applied through RTSS.
pub(super) struct Follow {
    /// Which stream's cap this is: a change on its way belongs to one.
    pub(super) id: u64,
    pub(super) root: PathBuf,
    pub(super) rate: Rate,
    /// The cap is off: the stream's display refreshes too slowly for it.
    pub(super) standing_down: bool,
    /// A change is on its way on its own thread.
    pub(super) changing: bool,
    /// A change failed; the cap stays as it is until the stream ends.
    pub(super) failed: bool,
    /// A second stream joined: the cap is no longer one display's to decide, and stays on
    /// until the last stream ends.
    pub(super) shared: bool,
}

pub(super) fn next_follow() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The stream's display refreshes at `display` now: take a virtual display's automatic cap
/// off while that is too slow for it ([`butterpollo_media_policy::framegen::cap_suits_display`]),
/// and put it back when the display is fast again. A cap the user asked for, a second
/// stream's and a host without RTSS are left alone. Called from the stream's loop with each
/// timing window, so it never waits: the change is made on a thread of its own.
pub fn follow_display(display: Option<Rate>) {
    let Some(display) = display else {
        return;
    };
    // A stream starting or ending holds the lock across its RTSS calls; the next window asks
    // again.
    let Ok(mut guard) = state().try_lock() else {
        return;
    };
    let Some(stand_down) = wanted_change(&guard, display) else {
        return;
    };
    let Some(follow) = guard.follow.as_mut() else {
        return;
    };
    let id = follow.id;
    // Started under the lock, which the thread takes first: it runs once this returns, and a
    // thread that could not start leaves nothing to undo.
    let spawned = std::thread::Builder::new()
        .name("limiter-display".into())
        .spawn(move || {
            let mut guard = state().lock().unwrap();
            let state = &mut *guard;
            // The stream may have ended, and another begun, since this was asked for.
            let Some(follow) = state.follow.as_mut().filter(|follow| follow.id == id) else {
                return;
            };
            follow.changing = false;
            if wanted_change(state, display) == Some(stand_down) {
                change_followed(state, stand_down, Some(display));
            }
        });
    match spawned {
        Ok(_) => follow.changing = true,
        Err(error) => {
            tracing::warn!(%error, "the frame limiter could not follow the display: no thread")
        }
    }
}

/// Whether a cap follows one stream's display now, so the stream reads that display's mode
/// every live tick (250 ms) rather than only with each 5 s timing window: a game that sets its
/// own mode takes the cap off within a tick. One mode read and an uncontended lock each
/// tick, none per frame, and nothing at all for a stream without a followed cap.
pub fn following() -> bool {
    state().try_lock().is_ok_and(|state| follows(&state))
}

pub(super) fn follows(state: &State) -> bool {
    state.users == 1
        && state.active == "rtss"
        && state
            .follow
            .as_ref()
            .is_some_and(|follow| !follow.failed && !follow.shared)
}

/// Whether the stream reads its display's mode at this live tick: with every timing window,
/// and between windows while a cap follows the display.
pub fn display_read_due(window_due: bool, following: bool) -> bool {
    window_due || following
}

/// Whether the followed cap has to come off (`true`) or go back on for a display at
/// `display`, or `None` when it is as it should be, a change is under way, or there is no
/// cap one stream's display decides.
pub(super) fn wanted_change(state: &State, display: Rate) -> Option<bool> {
    let follow = state.follow.as_ref()?;
    if state.users != 1
        || state.active != "rtss"
        || follow.changing
        || follow.failed
        || follow.shared
    {
        return None;
    }
    let stand_down = !butterpollo_media_policy::framegen::cap_suits_display(follow.rate, display);
    (stand_down != follow.standing_down).then_some(stand_down)
}

/// Take the followed cap off or put it back, in place: the journal keeps what RTSS held
/// before the stream, and the stream's end restores that either way.
pub(super) fn change_followed(state: &mut State, stand_down: bool, display: Option<Rate>) {
    let Some(follow) = state.follow.as_mut() else {
        return;
    };
    let result = retarget(&state.directory, &follow.root, stand_down);
    let display_hz = display.map(|rate| f64::from(rate.0) / 1000.);
    match result {
        Ok(()) => {
            follow.standing_down = stand_down;
            if stand_down {
                tracing::info!(rate = ?follow.rate.rational(), display_hz, "the stream's display now refreshes too slowly for the automatic frame limit, so the limit is off and the display's refresh paces the game");
            } else {
                tracing::info!(rate = ?follow.rate.rational(), display_hz, "the automatic frame limit is back on");
            }
        }
        Err(error) => {
            follow.failed = true;
            // RTSS may hold the new value without having confirmed it: go by its profile.
            let off = rtss::read(&follow.root)
                .and_then(|profile| rtss::properties(&profile))
                .is_ok_and(|current| {
                    no_limit()
                        .iter()
                        .all(|(key, value)| current.get(key) == Some(value))
                });
            follow.standing_down = off;
            tracing::warn!(stand_down, limit_off = off, display_hz, error = %format!("{error:#}"), "the automatic frame limit could not follow the display's refresh rate; RTSS keeps what it has now until the stream ends");
        }
    }
}

/// No limit, as RTSS holds it.
pub(super) fn no_limit() -> BTreeMap<String, Option<u32>> {
    BTreeMap::from([
        ("Limit".into(), Some(0)),
        ("LimitDenominator".into(), Some(1)),
    ])
}

/// Give RTSS no limit in place of the stream's cap (`stand_down`), or the cap again. RTSS
/// keeps running and the journal keeps the user's own values: a limit of theirs is the
/// stream's to replace until it ends, and would run beside the display's refresh as the
/// cap did (their 120 fps limit on a 60 Hz mode left 3.3 % of the frame intervals off, the
/// 2026-11-19 run). The journal names both values as this host's before RTSS gets the
/// first, so a host that dies in either state restores the user's. A limit someone changed
/// during the stream is theirs and is left alone.
fn retarget(directory: &Path, root: &Path, stand_down: bool) -> Result<()> {
    let mut journal = load(directory)?;
    let Some(change) = journal.rtss.as_mut().filter(|change| change.root == root) else {
        bail!("the stream's frame limit is no longer in the recovery journal");
    };
    let current = rtss::properties(&rtss::read(root)?)?;
    let wanted = if stand_down {
        no_limit()
    } else {
        change.applied.clone()
    };
    if let Some(key) = wanted.keys().find(|key| !change.ours(key, &current)) {
        bail!("RTSS's {key} was changed during the stream");
    }
    if stand_down && change.stood_down.is_none() {
        change.stood_down = Some(wanted.clone());
        persist(directory, &journal)?;
    }
    // Off leaves RTSS's own switch for its limiter alone; the cap needs it on.
    let reply = rtss::apply(root, &wanted, (!stand_down).then_some(false))?;
    if !stand_down && reply.flags & 4 != 0 {
        bail!("RTSS did not confirm the requested rational frame limit");
    }
    let unsupported = confirmed(&wanted, &reply.values, !stand_down)
        .map_err(|key| anyhow::anyhow!("RTSS did not confirm {key}"))?;
    warn_unsupported(&unsupported);
    // The cap is back, so no limit is no longer this host's value: one that somebody
    // turns off from here on is theirs. A journal that cannot be written keeps the claim.
    if !stand_down
        && journal
            .rtss
            .as_mut()
            .is_some_and(|change| change.stood_down.take().is_some())
    {
        let _ = persist(directory, &journal);
    }
    Ok(())
}
