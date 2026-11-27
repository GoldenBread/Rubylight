//! Dynamic RTSS frame-limit stand-down when a virtual display's refresh drops below the cap.
use super::{State, confirmed, load, persist, state, warn_unsupported};
use crate::rtss;
use anyhow::{Result, bail};
use butterpollo_config::config::Config;
use butterpollo_media_policy::framegen::{Rate, queue_clamp_step};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

static FOLLOWING: AtomicBool = AtomicBool::new(false);
static QUEUE_CLAMPING: AtomicBool = AtomicBool::new(false);

pub(super) fn sync_flags(state: &State) {
    let tracked = tracked_single_stream(state);
    let is_following = follows(state);
    let is_clamping = tracked
        && state
            .follow
            .as_ref()
            .is_some_and(|follow| follow.queue_clamp && !follow.standing_down);
    FOLLOWING.store(is_following, Ordering::Release);
    QUEUE_CLAMPING.store(is_clamping, Ordering::Release);
}

/// A single stream's dynamic RTSS cap state (`follow_display` and/or `frame_limiter_queue_clamp`).
pub(super) struct Follow {
    /// Which stream's cap this is: a change on its way belongs to one.
    pub(super) id: u64,
    pub(super) root: PathBuf,
    pub(super) rate: Rate,
    /// Whether slow display refresh modes stand down the automatic virtual-display cap.
    pub(super) follows_display: bool,
    /// The cap is off: the stream's display refreshes too slowly for it.
    pub(super) standing_down: bool,
    /// Whether `frame_limiter_queue_clamp` allows lowering the cap during GPU-bound dips.
    pub(super) queue_clamp: bool,
    /// Active queue-clamp cap applied in place of `rate`, or `None` at nominal.
    pub(super) clamped_rate: Option<Rate>,
    /// A change is on its way on its own thread.
    pub(super) changing: bool,
    /// A change failed; the cap stays as it is until the stream ends.
    pub(super) failed: bool,
    /// A second stream joined: the cap is no longer one display's to decide, and stays on
    /// until the last stream ends.
    pub(super) shared: bool,
}

pub(super) fn queue_clamp_enabled(config: &Config) -> bool {
    config.boolean("frame_limiter_queue_clamp", false)
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
/// own mode takes the cap off within a tick. Lock-free atomic read, updated under `state()`.
pub fn following() -> bool {
    FOLLOWING.load(Ordering::Acquire)
}

/// Whether the active stream's automatic cap has `frame_limiter_queue_clamp` enabled and is
/// not stood down for a slow display. Lock-free so background telemetry never sees false
/// negatives while RTSS updates hold `state()`.
pub fn queue_clamping() -> bool {
    QUEUE_CLAMPING.load(Ordering::Acquire)
}

fn tracked_single_stream(state: &State) -> bool {
    state.users == 1
        && state.active == "rtss"
        && state
            .follow
            .as_ref()
            .is_some_and(|follow| !follow.failed && !follow.shared)
}

pub(super) fn follows(state: &State) -> bool {
    tracked_single_stream(state)
        && state
            .follow
            .as_ref()
            .is_some_and(|follow| follow.follows_display)
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
    if !follows(state) || follow.changing {
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
                follow.clamped_rate = None;
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
    sync_flags(state);
}

/// Update RTSS's followed cap dynamically when `frame_limiter_queue_clamp` is enabled and the
/// stream is not stood down for a slow display: clamps to 97% of `rendered_fps_milli` during
/// GPU-bound dips and restores `nominal` once headroom returns.
pub fn follow_queue_clamp(rendered_fps_milli: u32, gpu_busy_percent: Option<u8>) {
    let Ok(mut guard) = state().try_lock() else {
        return;
    };
    let Some(next) = wanted_clamp(&guard, rendered_fps_milli, gpu_busy_percent) else {
        return;
    };
    let Some(follow) = guard.follow.as_mut() else {
        return;
    };
    let id = follow.id;
    let spawned = std::thread::Builder::new()
        .name("limiter-clamp".into())
        .spawn(move || {
            let mut guard = state().lock().unwrap();
            let state = &mut *guard;
            let Some(follow) = state.follow.as_mut().filter(|follow| follow.id == id) else {
                return;
            };
            follow.changing = false;
            if wanted_clamp(state, rendered_fps_milli, gpu_busy_percent) == Some(next) {
                change_clamped(state, next);
            }
        });
    match spawned {
        Ok(_) => follow.changing = true,
        Err(error) => {
            tracing::warn!(%error, "the frame limiter could not apply queue clamp: no thread")
        }
    }
}

/// Desired queue-clamp target when a single stream owns RTSS, `frame_limiter_queue_clamp`
/// is enabled, and the cap is not stood down; `None` if no change is needed.
pub(super) fn wanted_clamp(
    state: &State,
    rendered_fps_milli: u32,
    gpu_busy_percent: Option<u8>,
) -> Option<Option<Rate>> {
    let follow = state.follow.as_ref()?;
    if !tracked_single_stream(state)
        || !follow.queue_clamp
        || follow.standing_down
        || follow.changing
    {
        return None;
    }
    let next = queue_clamp_step(
        follow.rate,
        follow.clamped_rate,
        rendered_fps_milli,
        gpu_busy_percent,
    );
    (next != follow.clamped_rate).then_some(next)
}

pub(super) fn change_clamped(state: &mut State, clamped: Option<Rate>) {
    let Some(follow) = state.follow.as_mut() else {
        return;
    };
    match retarget_clamp(&state.directory, &follow.root, clamped) {
        Ok(()) => {
            follow.clamped_rate = clamped;
            if let Some(rate) = clamped {
                tracing::info!(nominal = ?follow.rate.rational(), clamped = ?rate.rational(), "dynamic queue clamp lowered RTSS frame limit for GPU-bound headroom");
            } else {
                tracing::info!(nominal = ?follow.rate.rational(), "dynamic queue clamp restored nominal RTSS frame limit");
            }
        }
        Err(error) => {
            follow.failed = true;
            tracing::warn!(clamped = ?clamped.map(Rate::rational), error = %format!("{error:#}"), "dynamic queue clamp could not update RTSS; keeping current limit until the stream ends");
        }
    }
    sync_flags(state);
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
    // Prune superseded host values only after RTSS confirmed the new profile state, so a
    // failed write or crash mid-transition still recognizes the previously live value.
    let pruned = journal.rtss.as_mut().is_some_and(|change| {
        if stand_down {
            change.clamped.take().is_some() | change.previous_clamped.take().is_some()
        } else {
            change.stood_down.take().is_some()
                | change.clamped.take().is_some()
                | change.previous_clamped.take().is_some()
        }
    });
    if pruned {
        let _ = persist(directory, &journal);
    }
    Ok(())
}

fn retarget_clamp(directory: &Path, root: &Path, clamped: Option<Rate>) -> Result<()> {
    let mut journal = load(directory)?;
    let Some(change) = journal.rtss.as_mut().filter(|change| change.root == root) else {
        bail!("the stream's frame limit is no longer in the recovery journal");
    };
    let current = rtss::properties(&rtss::read(root)?)?;
    let wanted: BTreeMap<String, Option<u32>> = match clamped {
        Some(rate) => {
            let (numerator, denominator) = rate.rational();
            BTreeMap::from([
                ("Limit".into(), Some(numerator)),
                ("LimitDenominator".into(), Some(denominator)),
            ])
        }
        None => BTreeMap::from([
            (
                "Limit".into(),
                change.applied.get("Limit").copied().flatten(),
            ),
            (
                "LimitDenominator".into(),
                change.applied.get("LimitDenominator").copied().flatten(),
            ),
        ]),
    };
    if let Some(key) = wanted.keys().find(|key| !change.ours(key, &current)) {
        bail!("RTSS's {key} was changed during the stream");
    }
    if clamped.is_some() {
        if let Some(prev) = change.clamped.take()
            && prev != wanted
        {
            change.previous_clamped = Some(prev);
        }
        change.clamped = Some(wanted.clone());
        persist(directory, &journal)?;
    }
    let reply = rtss::apply(root, &wanted, Some(false))?;
    if reply.flags & 4 != 0 {
        bail!("RTSS did not confirm the requested rational frame limit");
    }
    let unsupported = confirmed(&wanted, &reply.values, true)
        .map_err(|key| anyhow::anyhow!("RTSS did not confirm {key}"))?;
    warn_unsupported(&unsupported);
    let pruned = journal.rtss.as_mut().is_some_and(|change| {
        if clamped.is_none() {
            change.clamped.take().is_some() | change.previous_clamped.take().is_some()
        } else {
            change.previous_clamped.take().is_some()
        }
    });
    if pruned {
        let _ = persist(directory, &journal);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn followed_state(queue_clamp: bool) -> State {
        State {
            users: 1,
            active: "rtss".into(),
            rate: Some(Rate(120_000)),
            follow: Some(Follow {
                id: 1,
                root: PathBuf::new(),
                rate: Rate(120_000),
                follows_display: true,
                standing_down: false,
                queue_clamp,
                clamped_rate: None,
                changing: false,
                failed: false,
                shared: false,
            }),
            ..State::default()
        }
    }

    #[test]
    fn queue_clamp_setting_is_opt_in_and_respects_active_follow_guards() {
        assert!(!queue_clamp_enabled(&Config::default()));
        assert!(!queue_clamp_enabled(
            &Config::parse("frame_limiter_queue_clamp=false").unwrap()
        ));
        assert!(queue_clamp_enabled(
            &Config::parse("frame_limiter_queue_clamp=true").unwrap()
        ));

        // Disabled by default: never requests a dynamic clamp.
        let disabled = followed_state(false);
        assert_eq!(wanted_clamp(&disabled, 90_000, Some(99)), None);

        // Enabled and actively following: engages during GPU-bound dips and restores nominal.
        let mut enabled = followed_state(true);
        assert_eq!(
            wanted_clamp(&enabled, 90_000, Some(99)),
            Some(Some(Rate(87_300)))
        );
        enabled.follow.as_mut().unwrap().clamped_rate = Some(Rate(87_300));
        assert_eq!(wanted_clamp(&enabled, 87_300, Some(95)), None);
        assert_eq!(wanted_clamp(&enabled, 87_300, Some(92)), Some(None));
        assert_eq!(wanted_clamp(&enabled, 116_400, Some(97)), Some(None));

        // Stood down for a slow display, changing, failed, shared, or inactive: no clamp.
        let mut stood_down = followed_state(true);
        stood_down.follow.as_mut().unwrap().standing_down = true;
        assert_eq!(wanted_clamp(&stood_down, 90_000, Some(99)), None);

        let mut changing = followed_state(true);
        changing.follow.as_mut().unwrap().changing = true;
        assert_eq!(wanted_clamp(&changing, 90_000, Some(99)), None);

        let mut failed = followed_state(true);
        failed.follow.as_mut().unwrap().failed = true;
        assert_eq!(wanted_clamp(&failed, 90_000, Some(99)), None);

        let mut shared = followed_state(true);
        shared.follow.as_mut().unwrap().shared = true;
        assert_eq!(wanted_clamp(&shared, 90_000, Some(99)), None);

        let mut multi_user = followed_state(true);
        multi_user.users = 2;
        assert_eq!(wanted_clamp(&multi_user, 90_000, Some(99)), None);

        let mut inactive = followed_state(true);
        inactive.active = "none".into();
        assert_eq!(wanted_clamp(&inactive, 90_000, Some(99)), None);

        // Explicit physical-monitor RTSS caps (`follows_display == false`) do not stand down for
        // display refresh, yet still allow opt-in dynamic queue clamping for single-stream RTSS.
        let mut explicit_cap = followed_state(true);
        explicit_cap.follow.as_mut().unwrap().follows_display = false;
        assert!(!follows(&explicit_cap));
        assert_eq!(wanted_change(&explicit_cap, Rate(60_000)), None);
        assert_eq!(
            wanted_clamp(&explicit_cap, 90_000, Some(99)),
            Some(Some(Rate(87_300)))
        );
    }
}
