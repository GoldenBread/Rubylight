//! Window-target parsing and openness validation for single-window capture streams.
use anyhow::{Context, Result};
use butterpollo_config::config::Config;
use butterpollo_win_graphics::capture::WindowTarget;

/// One window streamed instead of a display: `output_name` set to
/// `window:<hwnd>/<pid>@<display>`, as the release harness does for its looped
/// game clip (e2e.py --content). Like a co-op player's window, no display is
/// prepared, changed or kept; a malformed target is refused, never streamed as
/// a display, and a window that closes fails the capture loudly. The target
/// must name the window's process: a handle written into the settings outlives
/// its window, and Windows gives it to the next one.
pub(super) fn configured_window(config: &Config) -> Result<Option<WindowTarget>> {
    let output = config.get("output_name", "").trim();
    let target = WindowTarget::parse(output)
        .context("output_name is neither a display nor a window:<hwnd>/<pid>@<display> target")?;
    if let Some(target) = &target {
        anyhow::ensure!(
            target.process.is_some(),
            "output_name names the window {target} without its process: use window:<hwnd>/<pid>@<display>"
        );
    }
    Ok(target)
}

/// A closed, minimised, reused or misplaced configured window refuses the
/// stream with the reason.
pub(super) fn configured_window_open(target: &WindowTarget) -> Result<()> {
    butterpollo_win_graphics::capture::check_window(target)
        .with_context(|| format!("output_name names the window {target}, which cannot be streamed"))
}
