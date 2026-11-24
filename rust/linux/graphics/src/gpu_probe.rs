//! A trivial GPU round trip that a stalled GPU holds back, for the stream's stall watch. Linux has no GPU device to issue it on yet, so a probe cannot be opened.
use anyhow::Result;

pub struct Probe(());
impl Probe {
    /// Opens a probe on the GPU showing `display`. Always an error on Linux.
    pub fn new(_display: &str) -> Result<Self> {
        Err(crate::unsupported("The GPU stall probe"))
    }
    /// Submits the query to the GPU. Does nothing on Linux.
    pub fn issue(&self) {}
    /// Whether the GPU has reached the query issued last. An error on Linux, which ends
    /// the watch that asked.
    pub fn done(&self) -> Result<bool> {
        Err(crate::unsupported("The GPU stall probe"))
    }
}
