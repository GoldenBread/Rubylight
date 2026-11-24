//! Tests for event-driven encoder output waits without 100 us polling clamps.
use super::*;
use butterpollo_win_graphics::timing::{Signal, Timer};
use std::sync::atomic::{AtomicUsize, Ordering};

struct FakeOutput {
    pending: bool,
    due: Option<Instant>,
    event: Option<Arc<Signal>>,
    event_calls: AtomicUsize,
}

impl FakeOutput {
    fn polling() -> Self {
        Self {
            pending: true,
            due: None,
            event: None,
            event_calls: AtomicUsize::new(0),
        }
    }
    fn with_event(event: Arc<Signal>) -> Self {
        Self {
            pending: true,
            due: None,
            event: Some(event),
            event_calls: AtomicUsize::new(0),
        }
    }
}

impl EncoderOutput for FakeOutput {
    fn poll(&mut self) -> Result<Vec<butterpollo_win_graphics::encoder::Encoded>> {
        Ok(vec![])
    }
    fn pending(&self) -> bool {
        self.pending
    }
    fn backlog(&self) -> usize {
        usize::from(self.pending)
    }
    fn backlog_limit(&self) -> usize {
        1
    }
    fn log_stall(&self) {}
    fn output_due(&self) -> Option<Instant> {
        self.due
    }
    fn output_event(&self) -> Option<&Signal> {
        self.event_calls.fetch_add(1, Ordering::Relaxed);
        self.event.as_deref()
    }
    fn poll_blocks(&self) -> bool {
        false
    }
}

#[test]
fn output_wait_skips_100us_poll_clamp_when_event_is_active_and_calls_event_once() -> Result<()> {
    let now = Instant::now();
    let deadline = now + Duration::from_millis(16);

    let signal = Arc::new(Signal::new()?);
    let event_encoder = FakeOutput::with_event(signal);
    let (until, event) = output_wait(Some(&event_encoder), now, deadline);
    assert_eq!(until, deadline, "event-driven encoder waits to deadline");
    assert!(event.is_some());
    assert_eq!(event_encoder.event_calls.load(Ordering::Relaxed), 1);

    let mut polling = FakeOutput::polling();
    let (until, event) = output_wait(Some(&polling), now, deadline);
    assert_eq!(until, now + OUTPUT_POLL, "polling-only keeps 100 us poll");
    assert!(event.is_none());

    polling.due = Some(now + Duration::from_millis(4));
    let (until, event) = output_wait(Some(&polling), now, deadline);
    assert_eq!(until, now + Duration::from_millis(4));
    assert!(event.is_none());

    let short = now + Duration::from_micros(40);
    let (until, event) = output_wait(Some(&polling), now, short);
    assert_eq!(until, short, "never exceeds deadline");
    assert!(event.is_none());
    Ok(())
}

#[test]
fn wait_for_output_uses_watchdog_bound_and_wakes_immediately_on_event() -> Result<()> {
    let timer = Timer::new()?;
    let recovery = EncoderRecovery::default();
    let signal = Arc::new(Signal::new()?);
    let encoder = FakeOutput::with_event(signal.clone());

    // Unset event waits until the 5 ms watchdog bound (not 100 us).
    let started = Instant::now();
    wait_for_output(&timer, &recovery, Some(&encoder))?;
    assert!(
        started.elapsed() >= Duration::from_millis(3),
        "expected watchdog wait, elapsed {:?}",
        started.elapsed()
    );

    // Signaling from another thread wakes wait_for_output immediately.
    let wake = signal.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1));
        let _ = wake.set();
    });
    let started = Instant::now();
    wait_for_output(&timer, &recovery, Some(&encoder))?;
    let elapsed = started.elapsed();
    handle.join().unwrap();
    assert!(
        elapsed < Duration::from_millis(50),
        "signaled event should wake promptly, elapsed {elapsed:?}"
    );
    Ok(())
}
