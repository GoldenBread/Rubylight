//! What the firewall does to inbound traffic for the host. Linux does not read its firewall yet, so nothing is reported in the way.

use anyhow::Result;

/// What the firewall does to inbound traffic for `program`: each problem as a sentence. Empty
/// when nothing stands in the way, or, as on Linux for now, when nothing is read.
pub fn problems(program: &std::path::Path) -> Result<Vec<String>> {
    let _ = program;
    Ok(Vec::new())
}
