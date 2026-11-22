//! Callback hooks notifying `butterpollo-win-display`'s recovery journal of audio and microphone route changes.
use anyhow::Result;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub struct RecoveryJournalHooks {
    pub audio: fn(bool) -> Result<()>,
    pub microphone: fn(bool) -> Result<()>,
}

static RECOVERY_HOOKS: OnceLock<RecoveryJournalHooks> = OnceLock::new();

pub fn set_recovery_hooks(hooks: RecoveryJournalHooks) {
    let _ = RECOVERY_HOOKS.set(hooks);
}

pub(super) fn mark_audio(pending: bool) -> Result<()> {
    RECOVERY_HOOKS
        .get()
        .map_or(Ok(()), |hooks| (hooks.audio)(pending))
}

pub(super) fn mark_microphone(pending: bool) -> Result<()> {
    RECOVERY_HOOKS
        .get()
        .map_or(Ok(()), |hooks| (hooks.microphone)(pending))
}
