//! Callback hook notifying `butterpollo-win-display`'s recovery journal of RTSS limiter changes.
use anyhow::Result;
use serde::Serialize;
use std::{path::Path, sync::OnceLock};

static RECOVERY_HOOK: OnceLock<fn(bool) -> Result<()>> = OnceLock::new();

pub fn set_recovery_hook(hook: fn(bool) -> Result<()>) {
    let _ = RECOVERY_HOOK.set(hook);
}

pub(super) fn mark_external(pending: bool) -> Result<()> {
    RECOVERY_HOOK.get().map_or(Ok(()), |hook| hook(pending))
}

pub(super) fn persist_json(file: &Path, value: &impl Serialize) -> Result<()> {
    butterpollo_session::state::write_json(file, value)?;
    mark_external(true)
}
