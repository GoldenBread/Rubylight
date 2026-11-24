//! Present times for captured frames. Linux has no source of an app's present times yet, so a frame keeps the time it was captured at.
use std::time::Instant;

/// Moves a frame's capture time onto the present that produced it, where presents are
/// tracked. On Linux none are.
#[derive(Default)]
pub struct Stamper(());
impl Stamper {
    /// Whether present times are tracked; without them `stamp` returns the capture time
    /// unchanged. Always false on Linux.
    pub fn tracks_presents(&self) -> bool {
        false
    }
    /// The capture time, unchanged.
    pub fn stamp(&mut self, captured: Instant, _output: &str) -> Instant {
        captured
    }
}
