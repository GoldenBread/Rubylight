//! Media-thread timers, wake signals, the process-wide streaming scope and the stream clock on Linux. Keeping the display awake and the Windows scheduling changes a stream makes (priority class, timer resolution, power throttling) are not available yet.
use anyhow::Result;
use std::{
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::{self, Thread},
    time::{Duration, Instant},
};

/// Paced waits start with this wake lead and never exceed the maximum.
const PACED_LEAD_INITIAL_US: u32 = 300;
const PACED_LEAD_MAX_US: u32 = 1000;
/// One timer per media worker. Its waits park the calling thread until a monotonic deadline,
/// which the kernel serves from its high-resolution timers. The field is the measured wake
/// lateness of paced waits, in microseconds.
pub struct Timer(AtomicU32);
/// Each capture consumer owns a separate signal. It stays set until it is reset, so resetting
/// it cannot consume another client's notification, and publishing before a wait is safe.
pub struct Signal {
    set: AtomicBool,
    /// The threads waiting on this signal now.
    waiters: Mutex<Vec<Thread>>,
}
/// Keeps capture's display awake for as long as it lives, on the thread that entered it.
/// Linux has no such request yet, so none is ever held.
pub struct DisplayAwake {
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl DisplayAwake {
    /// Always an error on Linux: the display's idle timeout is left as the desktop set it.
    pub fn enter() -> Result<Self> {
        Err(crate::unsupported("Keeping the display awake"))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Signal {
    pub fn new() -> Result<Self> {
        Ok(Self {
            set: AtomicBool::new(false),
            waiters: Mutex::new(Vec::new()),
        })
    }
    /// Sets the signal and wakes every thread waiting on it.
    pub fn set(&self) -> Result<()> {
        self.set.store(true, Ordering::SeqCst);
        // A waiter registers before it reads the flag, so one that missed the store is in the
        // list and gets its wake here.
        for waiter in lock(&self.waiters).iter() {
            waiter.unpark();
        }
        Ok(())
    }
    /// Whether the signal is set, without waiting.
    pub(crate) fn is_set(&self) -> bool {
        self.set.load(Ordering::SeqCst)
    }
    /// Reset while holding the protected capture-image lock, before waiting.
    pub fn reset(&self) -> Result<()> {
        self.set.store(false, Ordering::SeqCst);
        Ok(())
    }
}

/// The calling thread's entry in the waiter list of each signal it waits on.
struct Waiting<'a>(&'a [&'a Signal]);
impl<'a> Waiting<'a> {
    fn register(signals: &'a [&'a Signal]) -> Self {
        for signal in signals {
            lock(&signal.waiters).push(thread::current());
        }
        Self(signals)
    }
}
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let me = thread::current().id();
        for signal in self.0 {
            lock(&signal.waiters).retain(|waiter| waiter.id() != me);
        }
    }
}

impl Timer {
    pub fn new() -> Result<Self> {
        Ok(Self(AtomicU32::new(PACED_LEAD_INITIAL_US)))
    }
    pub fn until(&self, deadline: Instant) {
        sleep_until(deadline);
    }
    /// A deadline wait that never returns early. Packet pacing uses `until_paced` instead.
    pub fn until_precise(&self, deadline: Instant) {
        self.until(deadline);
    }
    /// Packet pacing without spinning: wait until the deadline less the wake lateness this
    /// timer has measured (an average, at most 1 ms), so wakes land on the deadline on
    /// average. Returns up to that lead early; the pacer's accumulated deadline keeps the
    /// average rate exact, and an early send earns no extra credit.
    pub fn until_paced(&self, deadline: Instant) {
        let _ = self.wait_paced(deadline, None);
    }
    pub fn until_paced_or_signal(&self, deadline: Instant, signal: &Signal) -> Result<bool> {
        self.wait_paced(deadline, Some(signal))
    }
    fn wait_paced(&self, deadline: Instant, signal: Option<&Signal>) -> Result<bool> {
        let lead = Duration::from_micros(u64::from(self.0.load(Ordering::Relaxed)));
        let Some(target) = deadline.checked_sub(lead) else {
            return Ok(false);
        };
        if target <= Instant::now() {
            return Ok(false);
        }
        if let Some(signal) = signal {
            if self.until_or_signal(target, signal)? {
                return Ok(true);
            }
        } else {
            self.until(target);
        }
        let late = Instant::now().saturating_duration_since(target).as_micros() as u32;
        self.0.store(
            paced_lead(self.0.load(Ordering::Relaxed), late),
            Ordering::Relaxed,
        );
        Ok(false)
    }
    /// Frame claims remain interruptible throughout their timer wait.
    pub fn until_or_signal_precise(&self, deadline: Instant, signal: &Signal) -> Result<bool> {
        self.until_or_signal(deadline, signal)
    }
    /// Wait for capture or the precise encoder/static-frame deadline, without a polling or
    /// spinning thread.
    pub fn until_or_signal(&self, deadline: Instant, signal: &Signal) -> Result<bool> {
        self.wait_until(deadline, &[signal])
    }
    /// Wait for `signal`, `extra` (such as an encoder's output event) or the
    /// deadline; true when either signal ended the wait.
    pub fn until_or_signals(
        &self,
        deadline: Instant,
        signal: &Signal,
        extra: Option<&Signal>,
    ) -> Result<bool> {
        match extra {
            Some(extra) => self.wait_until(deadline, &[signal, extra]),
            None => self.wait_until(deadline, &[signal]),
        }
    }
    /// A deadline wait that `event`, when there is one, ends early.
    pub fn until_or_event(&self, deadline: Instant, event: Option<&Signal>) -> Result<bool> {
        match event {
            Some(event) => self.wait_until(deadline, &[event]),
            None => {
                self.until(deadline);
                Ok(false)
            }
        }
    }
    /// Waits for the first of `signals` or the deadline; true when a signal ended the wait.
    /// A deadline already past returns false without looking at the signals; a signal set
    /// while the deadline passes during the wait still counts.
    fn wait_until(&self, deadline: Instant, signals: &[&Signal]) -> Result<bool> {
        if deadline.saturating_duration_since(Instant::now()).is_zero() {
            return Ok(false);
        }
        if signals.is_empty() {
            sleep_until(deadline);
            return Ok(false);
        }
        let _waiting = Waiting::register(signals);
        loop {
            if signals.iter().any(|signal| signal.is_set()) {
                return Ok(true);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            // A park may end early or without a wake; the loop rechecks both conditions.
            thread::park_timeout(remaining);
        }
    }
}
/// Sleeps until the monotonic deadline, however often the sleep is cut short.
fn sleep_until(deadline: Instant) {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        thread::sleep(remaining);
    }
}
/// One-eighth exponential average of measured lateness. A single long stall
/// moves the lead by at most 1/8 of the cap.
fn paced_lead(lead: u32, late_us: u32) -> u32 {
    let late = late_us.min(PACED_LEAD_MAX_US);
    ((lead * 7 + late) / 8).min(PACED_LEAD_MAX_US)
}
/// Held for the lifetime of a stream; the count of live scopes is process-wide. The first
/// scope runs the host's `on_first` action and the last one drops what it returned (the Wi-Fi
/// streaming mode). Linux changes no process scheduling, timer resolution or pointer setting
/// for a stream yet.
pub struct StreamingScope(());
struct Streams {
    count: usize,
    /// Optional guard returned by the host on the first stream's start.
    wlan: Option<Box<dyn Send>>,
}
static STREAMS: Mutex<Streams> = Mutex::new(Streams {
    count: 0,
    wlan: None,
});
/// The system's monotonic clock in milliseconds, the same for every process on the machine.
/// Zero if the clock is unavailable.
pub fn qpc_ms() -> f64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes only to the live local `now`.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } == 0 {
        now.tv_sec as f64 * 1000. + now.tv_nsec as f64 / 1_000_000.
    } else {
        0.
    }
}
/// Does nothing on Linux: the kernel does not throttle the timers of a process without a
/// window.
pub fn disable_power_throttling() {}
impl StreamingScope {
    pub fn enter() -> Self {
        Self::enter_with(|| None)
    }
    /// Enter a streaming scope; `on_first` runs once when the first active stream enters
    /// and its guard is dropped when the last active stream leaves (used for Wi-Fi scan mode).
    pub fn enter_with(on_first: impl FnOnce() -> Option<Box<dyn Send>>) -> Self {
        let mut streams = lock(&STREAMS);
        if streams.count == 0 {
            streams.wlan = on_first();
        }
        streams.count += 1;
        Self(())
    }
    /// How many streams hold a scope now.
    pub fn active() -> usize {
        lock(&STREAMS).count
    }
}
impl Drop for StreamingScope {
    fn drop(&mut self) {
        let mut streams = lock(&STREAMS);
        streams.count = streams.count.saturating_sub(1);
        if streams.count == 0 {
            drop(streams.wlan.take());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paced_lead_follows_lateness_and_stays_bounded() {
        assert_eq!(paced_lead(300, 300), 300);
        assert_eq!(paced_lead(0, 800), 100);
        assert_eq!(paced_lead(800, 0), 700);
        // One long stall moves the lead by at most an eighth of the cap.
        assert_eq!(paced_lead(0, u32::MAX), PACED_LEAD_MAX_US / 8);
        let mut lead = 0;
        for _ in 0..200 {
            lead = paced_lead(lead, u32::MAX);
        }
        assert!(lead <= PACED_LEAD_MAX_US);
    }

    #[test]
    fn paced_waits_return_no_earlier_than_their_lead() -> Result<()> {
        let timer = Timer::new()?;
        for _ in 0..20 {
            let deadline = Instant::now() + Duration::from_millis(2);
            timer.until_paced(deadline);
            let lead = Duration::from_micros(u64::from(PACED_LEAD_MAX_US));
            assert!(Instant::now() + lead >= deadline);
        }
        // An expired deadline returns at once.
        let start = Instant::now();
        timer.until_paced(start);
        assert!(start.elapsed() < Duration::from_millis(1));
        Ok(())
    }

    #[test]
    fn interrupted_paced_waits_preserve_the_adaptive_lead() -> Result<()> {
        let timer = Timer::new()?;
        let signal = Signal::new()?;
        for _ in 0..20 {
            let deadline = Instant::now() + Duration::from_millis(2);
            assert!(!timer.until_paced_or_signal(deadline, &signal)?);
            let lead = Duration::from_micros(u64::from(PACED_LEAD_MAX_US));
            assert!(Instant::now() + lead >= deadline);
        }
        let lead = timer.0.load(Ordering::Relaxed);
        signal.set()?;
        let start = Instant::now();
        assert!(timer.until_paced_or_signal(start + Duration::from_secs(2), &signal)?);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(timer.0.load(Ordering::Relaxed), lead);
        Ok(())
    }

    #[test]
    fn precise_waits_end_on_the_signal_or_at_the_deadline() -> Result<()> {
        let timer = Timer::new()?;
        let signal = Signal::new()?;
        let deadline = Instant::now() + Duration::from_millis(2);
        assert!(!timer.until_or_signal_precise(deadline, &signal)?);
        assert!(Instant::now() >= deadline);
        signal.set()?;
        let start = Instant::now();
        assert!(timer.until_or_signal_precise(start + Duration::from_secs(2), &signal)?);
        assert!(start.elapsed() < Duration::from_secs(1));
        signal.reset()?;
        assert!(!timer.until_or_signal(Instant::now() + Duration::from_millis(2), &signal)?);
        Ok(())
    }

    #[test]
    fn short_and_expired_deadlines_do_not_return_early() -> Result<()> {
        let timer = Timer::new()?;
        for wait in [
            Duration::ZERO,
            Duration::from_micros(25),
            Duration::from_millis(2),
        ] {
            let deadline = Instant::now() + wait;
            timer.until_precise(deadline);
            assert!(Instant::now() >= deadline);
        }
        Ok(())
    }

    #[test]
    fn a_signal_set_on_another_thread_ends_the_wait() -> Result<()> {
        let timer = Timer::new()?;
        let (signal, extra) = (Signal::new()?, Signal::new()?);
        let start = Instant::now();
        let woke = thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(20));
                let _ = extra.set();
            });
            timer.until_or_signals(start + Duration::from_secs(5), &signal, Some(&extra))
        })?;
        assert!(woke);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(lock(&signal.waiters).is_empty() && lock(&extra.waiters).is_empty());
        // The event stays set until it is reset; without one, the deadline ends the wait.
        assert!(timer.until_or_event(Instant::now() + Duration::from_secs(5), Some(&extra))?);
        assert!(!timer.until_or_event(Instant::now() + Duration::from_millis(2), None)?);
        Ok(())
    }

    #[test]
    fn the_first_scope_runs_the_action_and_the_last_drops_its_guard() {
        struct Flag(std::sync::Arc<AtomicBool>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = std::sync::Arc::new(AtomicBool::new(false));
        let before = StreamingScope::active();
        let flag = Flag(dropped.clone());
        let first = StreamingScope::enter_with(move || Some(Box::new(flag) as Box<dyn Send>));
        let second = StreamingScope::enter_with(|| panic!("only the first scope runs the action"));
        assert_eq!(StreamingScope::active(), before + 2);
        drop(first);
        assert!(!dropped.load(Ordering::SeqCst));
        drop(second);
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(StreamingScope::active(), before);
    }

    #[test]
    fn the_clock_runs_forward_in_milliseconds() {
        let first = qpc_ms();
        thread::sleep(Duration::from_millis(5));
        let second = qpc_ms();
        assert!(first > 0. && second - first >= 5. && second - first < 5000.);
    }
}
