//! Host-supplied recovery callbacks invoked by display recovery without coupling `display` to `io`.
use anyhow::{Result, anyhow};
use std::{path::Path, sync::OnceLock};

#[derive(Clone, Copy)]
pub struct RecoveryHooks {
    pub limiter: fn(&Path) -> Result<()>,
    pub audio: fn(&Path) -> Result<()>,
    pub microphone: fn(&Path) -> Result<()>,
}

static HOOKS: OnceLock<RecoveryHooks> = OnceLock::new();

pub fn set_hooks(hooks: RecoveryHooks) {
    let _ = HOOKS.set(hooks);
}

pub fn initialize_with(directory: &Path, hooks: RecoveryHooks) -> Result<Option<anyhow::Error>> {
    set_hooks(hooks);
    super::initialize(directory)
}

pub fn wait_and_recover_with(pid: u32, directory: &Path, hooks: RecoveryHooks) -> Result<()> {
    set_hooks(hooks);
    super::wait_and_recover(pid, directory)
}

pub(super) fn limiter(directory: &Path) -> Result<()> {
    HOOKS.get().map_or_else(
        || Err(anyhow!("frame limiter recovery callback is not registered")),
        |h| (h.limiter)(directory),
    )
}

pub(super) fn audio(directory: &Path) -> Result<()> {
    HOOKS.get().map_or_else(
        || Err(anyhow!("audio route recovery callback is not registered")),
        |h| (h.audio)(directory),
    )
}

pub(super) fn microphone(directory: &Path) -> Result<()> {
    HOOKS.get().map_or_else(
        || {
            Err(anyhow!(
                "microphone route recovery callback is not registered"
            ))
        },
        |h| (h.microphone)(directory),
    )
}
