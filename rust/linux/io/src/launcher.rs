//! The first-run and repeated-launch experience of the portable package. Linux has no launcher yet: starting it fails saying so.

use anyhow::Result;

/// Run the launcher.
pub fn run() -> Result<()> {
    Err(crate::unsupported("The first-run launcher"))
}

/// Tell the user why the launcher failed. With no window to show it in, it goes to the log.
pub fn show_error(error: &str) {
    tracing::error!(%error, "the launcher failed");
}
