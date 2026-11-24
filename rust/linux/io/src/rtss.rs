//! RivaTuner Statistics Server on Linux: the program does not run here, so its worker
//! process, which the host starts on request, reports that it is not available.

use anyhow::Result;

/// Run the RTSS worker that talks to the host over the pipe `name`, for the host process
/// `parent`. It cannot run on Linux.
pub fn worker(_name: &str, _parent: u32) -> Result<()> {
    Err(crate::unsupported("The RTSS worker"))
}
