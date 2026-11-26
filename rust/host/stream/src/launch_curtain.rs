//! The launch curtain on the host: while a game a device launched with `rlLaunchCurtain=1`
//! starts, the host looks at the PC every quarter of a second and tells the device where the
//! start is (`0x5548`, `stream/control/launch_curtain.rs`; `GET /session/curtain`), so the
//! device keeps its own calm screen over the stream until the game's window is in front. The
//! state machine is `butterpollo_session::launch_curtain`; the curtain never holds or slows
//! the stream, which runs underneath it as always.
//!
//! One look ([`tick`]) runs right after `launch_focus`'s on the housekeeping loop, which brings
//! a game's window back over its store client, and reads what that left: the game's windows,
//! the window in front, the launcher prompt `launcher_watch` found, and Windows' secure desktop.
//! As `launch_focus`, it never holds `current_app` across process enumeration, window reading or
//! image-path lookups, and drops a look whose app was replaced meanwhile.
use crate::process::curtain_window;
use crate::state::Shared;
use butterpollo_session::launch_curtain::{Curtain, Look};
use rubylight_native_api::curtain::LaunchCurtainSnapshot;
use rubylight_protocol::launch_curtain::{CurtainState, Phase, flags};
use std::{
    collections::BTreeSet,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

/// How often the PC is looked at while a curtain is up.
pub const EVERY: Duration = butterpollo_session::launch_curtain::FAST_EVERY;

/// The launch with a curtain, if any: one at a time, as one app runs.
#[derive(Default)]
pub struct Tracker {
    inner: Mutex<Option<Active>>,
    /// Grows with every change, so a control peer reads the state only when it changed.
    revision: AtomicU64,
}

struct Active {
    launch_id: String,
    /// The device that launched it (`state::device_uuid`).
    owner: String,
    /// The launched app's generation (`RunningApp::generation`).
    generation: String,
    started: Instant,
    curtain: Curtain,
}

/// Why a device may not lift the curtain.
#[derive(Debug, PartialEq, Eq)]
pub enum DismissRefused {
    /// No launch has a curtain.
    NoCurtain,
    /// Another device launched the game.
    NotOwner,
}

fn device(uuid: &str) -> &str {
    butterpollo_session::state::device_uuid(uuid)
}

impl Tracker {
    fn locked(&self) -> std::sync::MutexGuard<'_, Option<Active>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn bump(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    /// The revision, for a control peer that only reads a changed state.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// `launch_id`, by `owner`, started `generation`'s app: its curtain begins, replacing any
    /// other. `bypass` for a launch with no game window to wait for, lifted from the start.
    pub fn begin(&self, launch_id: &str, owner: &str, generation: String, bypass: bool) {
        *self.locked() = Some(Active {
            launch_id: launch_id.to_owned(),
            owner: device(owner).to_owned(),
            generation,
            started: Instant::now(),
            curtain: Curtain::new(bypass),
        });
        self.bump();
        tracing::info!(launch = launch_id, bypass, "launch curtain begins");
    }

    /// A resume by `owner` of `generation`'s app while its curtain is still up (a reconnect, a
    /// roam): the curtain follows the new launch. False when there is none to follow.
    pub fn rebind(&self, launch_id: &str, owner: &str, generation: &str) -> bool {
        let mut inner = self.locked();
        let Some(active) = inner.as_mut().filter(|active| {
            active.curtain.active()
                && active.generation == generation
                && active.owner == device(owner)
        }) else {
            return false;
        };
        active.launch_id = launch_id.to_owned();
        drop(inner);
        self.bump();
        true
    }

    /// Whether a curtain is up: the housekeeping loop then looks every [`EVERY`].
    pub fn up(&self) -> bool {
        self.locked()
            .as_ref()
            .is_some_and(|active| active.curtain.active())
    }

    /// The state for `launch_id`'s stream: its curtain's, or a lifted one for a launch without
    /// a curtain (a resume, a curtain replaced by a newer launch).
    pub fn state_for(&self, launch_id: &str) -> CurtainState {
        match self.locked().as_ref() {
            Some(active) if active.launch_id == launch_id => active.curtain.state().clone(),
            _ => CurtainState {
                phase: Phase::Ready,
                flags: flags::DESKTOP_OR_LAUNCHER,
                sequence: 1,
                ..Default::default()
            },
        }
    }

    /// The curtain still up for an app of `generation`, and when it began.
    fn waiting(&self) -> Option<(String, Instant)> {
        self.locked().as_ref().and_then(|active| {
            active
                .curtain
                .active()
                .then(|| (active.generation.clone(), active.started))
        })
    }

    /// Hands `look` to the curtain if it still follows `generation`'s app.
    fn apply(&self, generation: &str, look: &Look) {
        let mut inner = self.locked();
        let Some(active) = inner.as_mut().filter(|a| a.generation == generation) else {
            return;
        };
        let before = active.curtain.state().phase;
        if active.curtain.look(look) {
            let after = active.curtain.state().phase;
            if after != before {
                tracing::info!(
                    launch = %active.launch_id,
                    phase = ?after,
                    elapsed_ms = active.curtain.state().elapsed_ms,
                    reason = %active.curtain.state().reason,
                    "launch curtain"
                );
            }
            drop(inner);
            self.bump();
        }
    }

    /// `device` lifts the curtain for everyone; only the device that launched the game may.
    pub fn dismiss(&self, uuid: &str) -> Result<LaunchCurtainSnapshot, DismissRefused> {
        let mut inner = self.locked();
        let active = inner.as_mut().ok_or(DismissRefused::NoCurtain)?;
        if active.owner != device(uuid) {
            return Err(DismissRefused::NotOwner);
        }
        let changed = active.curtain.dismiss(active.started.elapsed());
        if changed {
            tracing::info!(launch = %active.launch_id, "launch curtain dismissed by the device");
        }
        let snapshot = active.curtain.snapshot(self.revision() + 1, true);
        drop(inner);
        self.bump();
        Ok(snapshot)
    }

    /// `GET /session/curtain` for `uuid`.
    pub fn snapshot(&self, uuid: &str) -> LaunchCurtainSnapshot {
        let revision = self.revision();
        match self.locked().as_ref() {
            Some(active) => active
                .curtain
                .snapshot(revision, active.owner == device(uuid)),
            None => LaunchCurtainSnapshot {
                revision,
                ..Default::default()
            },
        }
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A launch whose app is now running begins its curtain, if it asked for one
/// ([`crate::extensions::launch_curtain_stream`]). A resume, and an app with no game window to
/// wait for, begin one that is lifted from the start.
pub fn begin_for(h: &Shared, launch: &crate::state::Launch, resume: bool) {
    let Ok(config) = crate::stream::effective_config(h, launch) else {
        return;
    };
    if !crate::extensions::launch_curtain_stream(&config, launch) {
        return;
    }
    let (generation, waits) = match h.current_app.lock().unwrap().as_ref() {
        Some(app) => (app.generation.clone(), app.curtain_waits()),
        None => (String::new(), false),
    };
    if resume
        && h.curtain
            .rebind(&launch.id, &launch.client.uuid, &generation)
    {
        return;
    }
    h.curtain.begin(
        &launch.id,
        &launch.client.uuid,
        generation,
        resume || !waits,
    );
}

/// The housekeeping loop's next look at `every` after now, or [`EVERY`] while a curtain is up.
pub fn next_look(h: &Shared, every: Duration) -> Instant {
    Instant::now() + if h.curtain.up() { EVERY } else { every }
}

/// One look, from the housekeeping loop, after `launch_focus`'s.
pub fn tick(h: &Shared) {
    look(
        h,
        |snapshot| snapshot.candidate_pids().unwrap_or_default(),
        crate::launch_focus::game_windows,
        crate::launch_focus::is_store_client,
        butterpollo_win_io::input::secure_desktop_shown,
    );
}

/// One look, with the OS reads passed in; `current_app` is released across every one of them.
fn look(
    h: &Shared,
    pids_fn: impl FnOnce(&crate::process::FocusSnapshot) -> BTreeSet<u32>,
    read: impl FnOnce(&BTreeSet<u32>) -> (Vec<(u32, bool)>, Option<u32>),
    store_client: impl Fn(u32) -> bool,
    secure_desktop: impl FnOnce() -> bool,
) {
    let Some((generation, started)) = h.curtain.waiting() else {
        return;
    };
    // None: the launched app no longer runs, or another replaced it.
    let seen = {
        let Some(current) = h.try_current_app() else {
            return;
        };
        current
            .as_ref()
            .filter(|app| app.generation == generation)
            .map(|app| (app.curtain_snapshot(), app.owner.clone()))
    };
    let mut look = Look {
        elapsed: started.elapsed(),
        app_running: seen.is_some(),
        now_unix_ms: now_unix_ms(),
        ..Default::default()
    };
    if let Some((Some((snapshot, game_up)), _)) = &seen {
        look.game_up = *game_up;
        let pids = pids_fn(snapshot);
        let (windows, foreground) = if pids.is_empty() {
            (Vec::new(), None)
        } else {
            read(&pids)
        };
        look.window = curtain_window(&windows, foreground, &store_client);
    }
    look.secure_desktop = look.app_running && secure_desktop();
    look.launcher = seen
        .as_ref()
        .and_then(|(_, owner)| h.notes.launcher_notice(owner));
    h.curtain.apply(&generation, &look);
}

#[cfg(test)]
mod tests;
