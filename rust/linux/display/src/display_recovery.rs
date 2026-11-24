//! Restoration of display, audio and frame limiter settings after an abort or a crash, on
//! Linux.
//!
//! The Linux host keeps no recovery journal yet: nothing it does changes a setting that
//! would have to go back. Startup recovery therefore finds nothing pending, and a request to
//! journal a change fails so that no change is made without a way back.
use anyhow::Result;
use std::path::Path;

/// What recovery calls to put back settings other crates changed.
#[derive(Clone, Copy)]
pub struct RecoveryHooks {
    pub limiter: fn(&Path) -> Result<()>,
    pub audio: fn(&Path) -> Result<()>,
    pub microphone: fn(&Path) -> Result<()>,
}

const JOURNAL: &str = "The display recovery journal";

/// Take the journal in `directory` and undo what an interrupted host left behind, returning
/// what could not be restored. There is no journal on Linux, so nothing is left behind.
pub fn initialize_with(_directory: &Path, _hooks: RecoveryHooks) -> Result<Option<anyhow::Error>> {
    Ok(None)
}

/// Wait for the host `pid` to end and undo what it left behind (`--display-watch`). A Linux
/// host leaves nothing behind, so this returns at once.
pub fn wait_and_recover_with(_pid: u32, _directory: &Path, _hooks: RecoveryHooks) -> Result<()> {
    Ok(())
}

/// Whether startup recovery left restoration unfinished. Never on Linux.
pub fn has_pending() -> bool {
    false
}

/// Retry unfinished startup recovery. `Ok(false)`: nothing remains.
pub fn recover_pending() -> Result<bool> {
    Ok(false)
}

/// Undo everything the journal holds now. It holds nothing on Linux.
pub fn reset() -> Result<()> {
    Ok(())
}

/// Journal that the frame limiter of another program was changed (`pending`), or that it is
/// back. Clearing succeeds; recording fails, as Linux has no journal to record in.
pub fn external(pending: bool) -> Result<()> {
    record(pending)
}

/// Journal that the audio route was changed (`pending`), or that it is back. Clearing
/// succeeds; recording fails, as Linux has no journal to record in.
pub fn audio(pending: bool) -> Result<()> {
    record(pending)
}

/// Journal that the microphone route was changed (`pending`), or that it is back. Clearing
/// succeeds; recording fails, as Linux has no journal to record in.
pub fn microphone(pending: bool) -> Result<()> {
    record(pending)
}

/// Journal the saved layout a stream restores at its end, with the displays it leaves alone,
/// or clear it with `None`. Clearing succeeds; recording fails on Linux.
pub fn baseline(value: Option<(crate::display::Snapshot, Vec<String>)>) -> Result<()> {
    record(value.is_some())
}

fn record(pending: bool) -> Result<()> {
    if pending {
        return Err(crate::unsupported(JOURNAL));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks() -> RecoveryHooks {
        fn restored(_: &Path) -> Result<()> {
            Ok(())
        }
        RecoveryHooks {
            limiter: restored,
            audio: restored,
            microphone: restored,
        }
    }

    #[test]
    fn nothing_is_pending_and_nothing_can_be_journaled() {
        let directory = std::env::temp_dir();
        assert!(initialize_with(&directory, hooks()).unwrap().is_none());
        assert!(wait_and_recover_with(1, &directory, hooks()).is_ok());
        assert!(!has_pending() && !recover_pending().unwrap() && reset().is_ok());
        for journal in [external, audio, microphone] {
            assert!(journal(false).is_ok());
            assert!(journal(true).is_err());
        }
        assert!(baseline(None).is_ok());
    }
}
