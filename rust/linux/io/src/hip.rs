//! What the AMD compute runtime (HIP) reports about each GPU. Linux does not load it yet, so no GPU is reported and Neural Rendering is not offered.

use anyhow::Result;
use butterpollo_session::neural::Gpu;

/// Every GPU the runtime finds: none on Linux for now.
pub fn gpus() -> Result<Vec<Gpu>> {
    Ok(Vec::new())
}
