//! The session the host runs in. A Linux host runs as the signed-in user, so a "session" here is that user: its number is the user ID, the console's session is the host's own, and there is no disconnected desktop to return to the console.

use anyhow::{Context, Result};
use std::os::unix::fs::MetadataExt;

/// The session at the PC's screen: the host's own, as a number that never changes while
/// the host runs.
pub fn console_session() -> u32 {
    // SAFETY: getuid takes no arguments and cannot fail.
    unsafe { libc::getuid() }
}

/// This process's own session: the user it runs as.
pub fn current_session() -> Result<u32> {
    Ok(console_session())
}

/// The session of the process `pid`: the user it runs as, which is the owner of its
/// `/proc` entry.
pub fn process_session(pid: u32) -> Result<u32> {
    Ok(std::fs::metadata(format!("/proc/{pid}"))
        .with_context(|| format!("process {pid} is gone"))?
        .uid())
}

/// Whether the host's session can show a stream. The host runs in the user's own
/// session, so always.
pub fn ensure_streaming_session() -> Result<()> {
    Ok(())
}

/// Whether the host's desktop can be returned to the PC's screen.
#[derive(serde::Serialize)]
pub struct ConsoleRecovery {
    pub session_id: u32,
    pub console_session_id: u32,
    pub can_reattach: bool,
    pub reason: String,
}

/// What [`reattach_console`] could do: nothing on Linux, where the host's session is not
/// one a remote desktop took from the screen.
pub fn console_recovery() -> Result<ConsoleRecovery> {
    Ok(ConsoleRecovery {
        session_id: current_session()?,
        console_session_id: console_session(),
        can_reattach: false,
        reason: crate::unsupported("Returning a remote desktop session to the PC's screen")
            .to_string(),
    })
}

/// Returns the host's remote desktop session to the PC's screen, which a Linux host has
/// no way to do.
pub fn reattach_console(_expected_session: u32, _expected_console: u32) -> Result<()> {
    Err(crate::unsupported(
        "Returning a remote desktop session to the PC's screen",
    ))
}

/// Whether a user is signed in to this process's session: the host runs as that user.
pub fn user_signed_in() -> bool {
    true
}
