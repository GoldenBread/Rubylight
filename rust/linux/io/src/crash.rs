//! Crash reports on Linux: a panic is written to the log and to `crashes/panic.txt` in the profile folder. No minidump is made yet, and a crash that is not a panic leaves no report.

use anyhow::{Result, ensure};
use std::{path::Path, sync::OnceLock};

static STARTED: OnceLock<()> = OnceLock::new();

/// Report panics from now on, into the `crashes` folder of `directory`. Fails when it was
/// done before.
pub fn initialize(directory: &Path) -> Result<()> {
    let directory = directory.join("crashes");
    std::fs::create_dir_all(&directory)?;
    ensure!(
        STARTED.set(()).is_ok(),
        "crash reporting already initialized"
    );
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(panic = %info, "the host panicked");
        let _ = std::fs::write(directory.join("panic.txt"), info.to_string());
        previous(info);
    }));
    Ok(())
}

/// The helper process that writes minidumps on Windows. Linux starts none, so there is
/// nothing for it to do.
pub fn reporter() -> Result<()> {
    Ok(())
}
