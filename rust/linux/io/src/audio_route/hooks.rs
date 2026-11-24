//! The callbacks that tell the display crate's recovery journal about audio and microphone
//! route changes.
use anyhow::Result;
use std::sync::OnceLock;

/// The journal's callbacks, each told whether a change is pending.
#[derive(Clone, Copy)]
pub struct RecoveryJournalHooks {
    pub audio: fn(bool) -> Result<()>,
    pub microphone: fn(bool) -> Result<()>,
}

static RECOVERY_HOOKS: OnceLock<RecoveryJournalHooks> = OnceLock::new();

/// Register the journal's callbacks; only the first registration counts.
pub fn set_recovery_hooks(hooks: RecoveryJournalHooks) {
    let _ = RECOVERY_HOOKS.set(hooks);
}

/// Tell the journal whether an audio route change is pending; succeeds when none are set.
pub fn mark_audio(pending: bool) -> Result<()> {
    RECOVERY_HOOKS
        .get()
        .map_or(Ok(()), |hooks| (hooks.audio)(pending))
}

/// Tell the journal whether a microphone route change is pending; succeeds when none are set.
pub fn mark_microphone(pending: bool) -> Result<()> {
    RECOVERY_HOOKS
        .get()
        .map_or(Ok(()), |hooks| (hooks.microphone)(pending))
}
