//! The callback that tells the display crate's recovery journal about frame limiter changes.
use anyhow::Result;
use std::sync::OnceLock;

static RECOVERY_HOOK: OnceLock<fn(bool) -> Result<()>> = OnceLock::new();

/// Register the journal's callback; only the first registration counts.
pub fn set_recovery_hook(hook: fn(bool) -> Result<()>) {
    let _ = RECOVERY_HOOK.set(hook);
}

/// Tell the journal whether a limiter change is pending; succeeds when no callback is set.
pub fn mark_external(pending: bool) -> Result<()> {
    RECOVERY_HOOK.get().map_or(Ok(()), |hook| hook(pending))
}
