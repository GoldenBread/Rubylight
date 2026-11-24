//! Console and Nightjar status assembly for Neural Rendering.
use super::{
    GAME_WAIT, KEY, LEGACY_KEY, Looked,
    game_check::{app, game_check},
    install::running,
    probe::{gpu_capability, system_missing, without},
    record::accepted,
};
use crate::state::Shared;
use butterpollo_session::neural::{self, StatusInput};
use serde_json::Value;
use std::time::Duration;

/// The switch as the console and Nightjar show it for one app. Blocks (the
/// first call checks the GPU).
pub fn status(h: &Shared, uuid: &str) -> Value {
    #[cfg(any(test, feature = "test-support"))]
    let wait = h.neural.game_wait.lock().unwrap().unwrap_or(GAME_WAIT);
    #[cfg(not(any(test, feature = "test-support")))]
    let wait = GAME_WAIT;
    status_waiting(h, uuid, wait)
}

/// Whether the host has an app `uuid`.
pub fn found(h: &Shared, uuid: &str) -> bool {
    app(h, uuid).is_some()
}

/// [`status`], waiting for the game's own check up to `wait`: a write's
/// answer waits for none.
pub fn status_waiting(h: &Shared, uuid: &str, wait: Duration) -> Value {
    let app = app(h, uuid);
    // Only what is held and System32: the game's folder, which may be on a
    // share that stalls, is looked into by the game's own check's thread.
    let gpu = gpu_capability(h, true);
    let system = system_missing(h);
    // The game's own check, only where it decides something: the switch is
    // off, nothing runs for the game, no stream (the check's helper would
    // take its disk and CPU) and the PC can run frame generation.
    let idle = !running(&h.neural.lock(), uuid);
    let streaming = crate::updater::streaming(h);
    let looked = app.as_ref().map_or_else(Looked::default, |app| {
        let ask =
            neural::should_run_game_check(idle, streaming, gpu.is_ok(), app.extra.contains_key(KEY));
        game_check(h, uuid, app, &system, wait, ask, streaming)
    });
    // Until the game's folder is looked into, the runtime is not known to
    // be missing: a game may carry it beside its program.
    let missing = looked
        .beside
        .as_deref()
        .map_or_else(Vec::new, |beside| without(system, beside));
    let capability = gpu.map_err(|reason| ("gpu", reason)).and_then(|capable| {
        neural::runtime(&missing)
            .map(|()| capable)
            .map_err(|reason| ("runtime", reason))
    });
    let game = looked.check.filter(|_| {
        idle && capability.is_ok() && app.as_ref().is_some_and(|app| !app.extra.contains_key(KEY))
    });
    let state = h.neural.lock();
    let setting = app.as_ref().and_then(|app| app.extra.get(KEY));
    let legacy = app.as_ref().and_then(|app| app.extra.get(LEGACY_KEY));
    neural::status_json(StatusInput {
        accepted: accepted(h),
        capability: match &capability {
            Ok(capable) => Ok(capable),
            Err((why, reason)) => Err((why, reason.as_str())),
        },
        found: app.is_some(),
        setting,
        legacy,
        game: game.as_ref(),
        job: state.jobs.get(uuid),
    })
}
