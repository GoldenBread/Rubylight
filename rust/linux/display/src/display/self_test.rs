//! The display self-test (`--display-self-test`). It exercises virtual displays and layout
//! changes, neither of which Linux has yet, so it reports that instead of running.
use anyhow::Result;

/// Run the checks and write `report`; true when every check passed. Fails on Linux before
/// anything is touched or written.
pub fn run(_report: &std::path::Path) -> Result<bool> {
    Err(crate::unsupported("The display self-test"))
}
