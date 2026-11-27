//! Endpoint change notifications for the audio route upkeep. A Linux host has no listener on
//! PipeWire's sinks yet, so its upkeep runs on the one-second poll alone, as it did before
//! Windows had them; the watch is held and wakes nothing.
use anyhow::Result;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

/// The upkeep's safety-net poll: it still runs this often with no event.
pub const POLL: Duration = Duration::from_secs(1);
/// The shortest gap between two upkeep rounds, so a burst of notifications cannot keep the
/// upkeep asking in a tight loop.
pub const MIN_GAP: Duration = Duration::from_millis(100);

/// What the upkeep runs when an endpoint changes.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A registered listener; none listens on Linux yet.
pub struct EndpointWatch {
    _wake: Wake,
}

impl EndpointWatch {
    /// Keep `wake` for the watch's lifetime. Nothing calls it on Linux: the poll finds changes.
    pub fn new(wake: Wake) -> Result<Self> {
        Ok(Self { _wake: wake })
    }
}

/// Wait for the next upkeep round: until a wake unparks this thread or [`POLL`] passes, then
/// at least [`MIN_GAP`] after `last_round`. A wake during the gap is kept by the park token and
/// runs one more round after it.
pub fn park_until_round(last_round: Instant) {
    thread::park_timeout(POLL);
    let since = last_round.elapsed();
    if since < MIN_GAP {
        thread::sleep(MIN_GAP - since);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_waits_the_gap_and_a_wake_ends_the_poll() {
        let started = Instant::now();
        thread::current().unpark();
        park_until_round(started);
        let waited = started.elapsed();
        assert!(waited >= MIN_GAP && waited < POLL, "{waited:?}");
        let _watch = EndpointWatch::new(Arc::new(|| {})).unwrap();
    }
}
