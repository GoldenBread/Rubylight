//! Unit tests for PyroWave probe retries after timeouts, crashes, missing sessions, and active streams.

use crate::state::test_support::Fixture;
use butterpollo_config::config::Config;
use butterpollo_win_graphics::codec_probe::{Crashed, Interrupted, SessionNotReady, TimedOut};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

const PYROWAVE: u32 = 0x0080_0000;

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_timeout_is_probed_again_exactly_once() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    fn probe(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
        CALLS.fetch_add(1, Ordering::AcqRel);
        Err(TimedOut.into())
    }
    let f = Fixture::new();
    let config = f.host.config.read().unwrap().clone();
    assert_eq!(
        f.host.retry_pyrowave_probe(&config, Duration::ZERO, probe),
        1
    );
    assert_eq!(CALLS.load(Ordering::Acquire), 1);
    assert_eq!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
}

/// Issue #94: the startup probe and two more before PyroWave stays
/// off, and a later one that passes offers it.
#[test]
fn a_crashed_worker_is_probed_twice_more() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    fn crashes(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
        CALLS.fetch_add(1, Ordering::AcqRel);
        Err(Crashed::new("exited before replying (0xc0000005)").into())
    }
    let f = Fixture::new();
    let config = f.host.config.read().unwrap().clone();
    assert_eq!(
        f.host
            .retry_pyrowave_probe(&config, Duration::ZERO, crashes),
        2
    );
    assert_eq!(CALLS.load(Ordering::Acquire), 2);
    assert_eq!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);

    static SECOND: AtomicU32 = AtomicU32::new(0);
    fn crashes_once(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
        if SECOND.fetch_add(1, Ordering::AcqRel) == 0 {
            return Err(Crashed::new("exited before replying (0xc0000005)").into());
        }
        Ok(PYROWAVE)
    }
    let f = Fixture::new();
    assert_eq!(
        f.host
            .retry_pyrowave_probe(&config, Duration::ZERO, crashes_once),
        2
    );
    assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
}

#[test]
fn a_session_not_ready_is_probed_until_it_is() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    fn probe(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
        if CALLS.fetch_add(1, Ordering::AcqRel) < 2 {
            return Err(SessionNotReady::new("nobody is signed in").into());
        }
        Ok(PYROWAVE)
    }
    let f = Fixture::new();
    let config = f.host.config.read().unwrap().clone();
    assert_eq!(
        f.host.retry_pyrowave_probe(&config, Duration::ZERO, probe),
        3
    );
    assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
}

#[test]
fn a_stream_that_starts_stops_the_probe_until_it_ends() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    static PROBING: AtomicBool = AtomicBool::new(false);
    fn probe(_: &Config, stop: &dyn Fn() -> bool) -> anyhow::Result<u32> {
        if CALLS.fetch_add(1, Ordering::AcqRel) > 0 {
            return Ok(PYROWAVE);
        }
        PROBING.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !stop() {
            anyhow::ensure!(
                Instant::now() < deadline,
                "the stream did not stop the probe"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(Interrupted.into())
    }
    let f = Fixture::new();
    let config = f.host.config.read().unwrap().clone();
    let host = f.host.clone();
    let retry =
        std::thread::spawn(move || host.retry_pyrowave_probe(&config, Duration::ZERO, probe));
    until("the probe started", || PROBING.load(Ordering::Acquire));
    f.host
        .sessions
        .lock()
        .unwrap()
        .teardown
        .insert("stream".into(), Instant::now());
    until("the stream stopped the probe", || {
        f.host.optional_probe.try_lock().is_ok()
    });
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        CALLS.load(Ordering::Acquire),
        1,
        "no probe while the stream runs"
    );
    f.host.sessions.lock().unwrap().teardown.clear();
    // The stopped probe was no attempt.
    assert_eq!(retry.join().unwrap(), 1);
    assert_eq!(CALLS.load(Ordering::Acquire), 2);
    assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
}

/// Streams that stop the probe use up none of its six attempts: one
/// more stream no longer leaves PyroWave off until a restart.
#[test]
fn streams_that_stop_the_probe_use_up_no_attempt() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    fn probe(_: &Config, _: &dyn Fn() -> bool) -> anyhow::Result<u32> {
        if CALLS.fetch_add(1, Ordering::AcqRel) < 7 {
            return Err(Interrupted.into());
        }
        Ok(PYROWAVE)
    }
    let f = Fixture::new();
    let config = f.host.config.read().unwrap().clone();
    assert_eq!(
        f.host.retry_pyrowave_probe(&config, Duration::ZERO, probe),
        1
    );
    assert_eq!(CALLS.load(Ordering::Acquire), 8);
    assert_ne!(f.host.codecs.load(Ordering::Acquire) & PYROWAVE, 0);
}

/// A stream that stops a codec pass's PyroWave probe, or starts
/// before it, completes the pass with the PyroWave modes found before
/// and a probe after the stream: the "No video encoder" retry ends
/// and the standard codecs are not probed again.
#[test]
fn a_stream_that_stops_the_pass_probe_keeps_its_modes_and_probes_after() {
    let (flags, retry) = crate::state::pyrowave_pass(Err(Interrupted.into()), PYROWAVE, true);
    assert_eq!((flags, retry), (PYROWAVE, Some(Duration::from_secs(5))));
    let (flags, retry) = crate::state::pyrowave_pass(Ok(PYROWAVE), 0, true);
    assert_eq!((flags, retry), (PYROWAVE, None));
    let (flags, retry) = crate::state::pyrowave_pass(Err(TimedOut.into()), PYROWAVE, true);
    assert_eq!((flags, retry), (0, Some(Duration::from_secs(60))));
    let failed = crate::state::pyrowave_pass(Err(anyhow::anyhow!("no DLL")), PYROWAVE, true);
    assert_eq!(failed, (0, None));
}
