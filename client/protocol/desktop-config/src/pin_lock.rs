//! PIN lock policy re-exports from `nightjar-policy::pin` plus an in-memory `StubPinStore`
//! for `nightjar-desktop-ui` tests and headless demos without pulling `ring` or platform SDKs.

#[cfg(any(test, feature = "demo-library", feature = "test-support"))]
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "demo-library", feature = "test-support"))]
use std::sync::Mutex;

pub use nightjar_policy::pin::{
    Clock, DEFAULT_GRACE_MINUTES, FILE, GRACE_CHOICES, GRANT_MS, HAND_OVER_MS, HandOver,
    MAX_DIGITS, MIN_DIGITS, Mode, Outcome, PinCrypto, PinGate, PinStoreBackend, Problem, RETRY_MS,
    Saved, View, app_needed, contains, grace_of, stream_key, stream_needed, throttle,
    within_grace,
};

/// Lightweight in-memory [`PinStoreBackend`] for `nightjar-desktop-ui` tests and fallback demos.
/// Production `nightjar-desktop` registers `nightjar_core::pin_lock::PinStore` via
/// `nightjar_desktop_ui::pin_ui::register_backend`.
#[cfg(any(test, feature = "demo-library", feature = "test-support"))]
pub struct StubPinStore {
    path: PathBuf,
    saved: Mutex<Saved>,
    problem: Mutex<Option<Problem>>,
}

#[cfg(any(test, feature = "demo-library", feature = "test-support"))]
impl StubPinStore {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE),
            saved: Mutex::new(Saved::default()),
            problem: Mutex::new(None),
        }
    }

    pub fn damaged(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE),
            saved: Mutex::new(Saved::closed()),
            problem: Mutex::new(Some(Problem::Damaged("expected value".into()))),
        }
    }
}

#[cfg(any(test, feature = "demo-library", feature = "test-support"))]
impl PinStoreBackend for StubPinStore {
    fn path(&self) -> &Path {
        &self.path
    }

    fn problem(&self) -> Option<Problem> {
        self.problem.lock().unwrap().clone()
    }

    fn has_pin(&self) -> bool {
        self.saved.lock().unwrap().hash.is_some()
    }

    fn view(&self) -> View {
        let saved = self.saved.lock().unwrap();
        let has_pin = saved.hash.is_some();
        View {
            has_pin,
            mode: if has_pin { saved.mode } else { Mode::Off },
            pcs: saved.pcs.clone(),
            grace_minutes: grace_of(saved.grace_min),
        }
    }

    fn pin_length(&self) -> usize {
        let saved = self.saved.lock().unwrap();
        if saved.hash.is_some() {
            saved.length.clamp(MIN_DIGITS, MAX_DIGITS)
        } else {
            0
        }
    }

    fn mode(&self) -> Mode {
        let saved = self.saved.lock().unwrap();
        if saved.hash.is_some() {
            saved.mode
        } else {
            Mode::Off
        }
    }

    fn set_mode(&self, mode: Mode) {
        if self.problem.lock().unwrap().is_none() {
            self.saved.lock().unwrap().mode = mode;
        }
    }

    fn set_pcs(&self, pcs: Vec<String>) {
        if self.problem.lock().unwrap().is_none() {
            self.saved.lock().unwrap().pcs = pcs;
        }
    }

    fn grace_minutes(&self) -> u32 {
        grace_of(self.saved.lock().unwrap().grace_min)
    }

    fn set_grace_minutes(&self, minutes: u32) {
        if self.problem.lock().unwrap().is_none() {
            self.saved.lock().unwrap().grace_min = minutes;
        }
    }

    fn set_pin(&self, pin: &[u8]) -> Result<(), String> {
        if !(MIN_DIGITS..=MAX_DIGITS).contains(&pin.len()) || !pin.iter().all(u8::is_ascii_digit) {
            return Err(format!("a PIN has {MIN_DIGITS} to {MAX_DIGITS} digits"));
        }
        if self.problem.lock().unwrap().is_some() {
            return Err("the PIN lock's file cannot be used".into());
        }
        let text = std::str::from_utf8(pin).map_err(|_| "bad utf-8".to_owned())?;
        let mut saved = self.saved.lock().unwrap();
        saved.hash = Some(format!("v1$1000$stub${text}"));
        saved.length = pin.len();
        saved.fails = 0;
        saved.lock_until = 0;
        saved.lock_boot = 0;
        Ok(())
    }

    fn remove_pin(&self) {
        if self.problem.lock().unwrap().is_none() {
            *self.saved.lock().unwrap() = Saved::default();
        }
    }

    fn locked_for_ms(&self) -> i64 {
        let saved = self.saved.lock().unwrap();
        let state = throttle::State {
            fails: saved.fails,
            lock_until: saved.lock_until,
            lock_boot: saved.lock_boot,
        };
        throttle::remaining_ms(state, self.elapsed())
    }

    fn try_pin(&self, pin: &[u8]) -> Outcome {
        if self.problem.lock().unwrap().is_some() {
            return Outcome::Unavailable;
        }
        let now = self.elapsed();
        let boot = self.boot();
        let mut saved = self.saved.lock().unwrap();
        let state = throttle::State {
            fails: saved.fails,
            lock_until: saved.lock_until,
            lock_boot: saved.lock_boot,
        };
        let locked = throttle::remaining_ms(state, now);
        if locked > 0 {
            return Outcome::Locked {
                locked_for_ms: locked,
            };
        }
        let ok = saved
            .hash
            .as_deref()
            .is_some_and(|h| h.split('$').nth(3) == std::str::from_utf8(pin).ok());
        if ok {
            saved.fails = 0;
            saved.lock_until = 0;
            saved.lock_boot = 0;
            Outcome::Ok
        } else {
            let counted = throttle::failed(state, now, boot);
            saved.fails = counted.fails;
            saved.lock_until = counted.lock_until;
            saved.lock_boot = counted.lock_boot;
            Outcome::Wrong {
                locked_for_ms: throttle::remaining_ms(counted, now),
            }
        }
    }

    fn set_active(&self, elapsed: i64, boot: i64) {
        if self.problem.lock().unwrap().is_none() {
            let mut saved = self.saved.lock().unwrap();
            saved.active_at = elapsed;
            saved.active_boot = boot;
        }
    }

    fn active_at(&self) -> i64 {
        self.saved.lock().unwrap().active_at
    }

    fn active_boot(&self) -> i64 {
        self.saved.lock().unwrap().active_boot
    }

    fn is_this_boot(&self, stored: i64) -> bool {
        stored == self.boot()
    }

    fn hand_over(&self) -> Result<Option<String>, String> {
        if !self.has_pin() {
            return Ok(None);
        }
        Ok(Some("01".repeat(32)))
    }

    fn end_hand_over(&self) {}

    fn handed_over(&self, _token: &str) -> bool {
        false
    }

    fn elapsed(&self) -> i64 {
        1_000_000
    }

    fn boot(&self) -> i64 {
        7
    }

    fn flush(&self) {}
}
