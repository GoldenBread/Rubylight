//! Pure connection-test outcome and launcher callback hook, decoupled from TCP/UDP socket probing.

use std::sync::mpsc::{channel, Receiver};
use std::sync::OnceLock;

use crate::hosts::Waker;

/// What the connectivity test found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every port answered.
    Clear,
    /// At least one did not: streaming over the internet may be blocked.
    Blocked,
    /// The test server could not be reached, so nothing is known.
    Unknown,
}

static START_HOOK: OnceLock<fn(Waker) -> Receiver<Outcome>> = OnceLock::new();

/// Registers the real TCP/UDP connectivity test worker from `nightjar-desktop::conn_test`.
pub fn register_start_hook(hook: fn(Waker) -> Receiver<Outcome>) {
    let _ = START_HOOK.set(hook);
}

/// Starts the connectivity test via the registered hook, or returns `Outcome::Unknown` in pure UI tests.
pub fn start(waker: Waker) -> Receiver<Outcome> {
    if let Some(hook) = START_HOOK.get() {
        return hook(waker);
    }
    let (tx, rx) = channel();
    let _ = tx.send(Outcome::Unknown);
    waker();
    rx
}
