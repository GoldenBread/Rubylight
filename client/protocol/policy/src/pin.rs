//! Pure PIN lock policy, lockout backoff, gate state machine and backend traits shared by
//! Nightjar Android and Nightjar Desktop. Zero `ring`, zero filesystem I/O and zero OS clocks.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub const MIN_DIGITS: usize = 4;
pub const MAX_DIGITS: usize = 8;
pub const DEFAULT_GRACE_MINUTES: u32 = 10;
pub const GRACE_CHOICES: [u32; 5] = [0, 5, 10, 30, 60];
pub const FILE: &str = "pin_lock.json";
/// How long the PIN screen's approval of one PC's stream start stays valid.
pub const GRANT_MS: i64 = 60_000;
/// How long a hand-over stays good: the second screen's process reads it as it starts.
pub const HAND_OVER_MS: i64 = 30_000;
/// How often a file with a [`Problem`] is read again.
pub const RETRY_MS: u64 = 1_000;

/// Cryptographic backend for salted PIN hashing, calibration and hand-over tokens.
pub trait PinCrypto: Send + Sync {
    fn create(&self, pin: &[u8], iterations: u32) -> Result<String, String>;
    fn verify(&self, stored: &str, pin: &[u8]) -> bool;
    fn well_formed(&self, stored: &str) -> bool;
    fn calibrate(&self) -> u32;
    /// A random 64-character hex token for second-screen hand-over.
    fn random_token(&self) -> Result<String, String>;
    fn token_hash(&self, token: &str) -> String;
}

/// The clock the lockout and the grace time run on; replaced in tests or supplied from Java.
pub trait Clock: Send + Sync {
    /// Milliseconds since the device started, sleep included.
    fn elapsed(&self) -> i64;
    /// A number for this start of the device.
    fn boot(&self) -> i64;
    /// Whether `stored`, a [`Self::boot`] saved earlier, was this start of the device.
    fn is_this_boot(&self, stored: i64) -> bool {
        stored == self.boot()
    }
}

/// Backoff after wrong PINs. Four tries are free; the fifth wrong one locks for 30 s and each
/// further one doubles that, up to 30 min. Times are on the time since boot, which a clock change
/// cannot move; a restart begins that time again, so the stored lock is then renewed at its
/// current step rather than dropped.
pub mod throttle {
    pub const FREE_FAILURES: u32 = 4;
    pub const BASE_MS: i64 = 30_000;
    pub const MAX_MS: i64 = 30 * 60_000;

    /// Wrong tries in a row, when the lock ends and on which boot that time counts.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct State {
        pub fails: u32,
        pub lock_until: i64,
        pub lock_boot: i64,
    }

    /// The wait that follows the `fails`-th wrong try in a row.
    pub fn delay_after(fails: u32) -> i64 {
        if fails <= FREE_FAILURES {
            return 0;
        }
        let doublings = (fails - FREE_FAILURES - 1).min(16);
        MAX_MS.min(BASE_MS << doublings)
    }

    /// The state seen from boot `boot`: a lock written on another boot (`same_boot` false) is
    /// renewed at its step.
    pub fn on_boot(s: State, now: i64, boot: i64, same_boot: bool) -> State {
        if same_boot {
            return s;
        }
        let delay = delay_after(s.fails);
        State {
            fails: s.fails,
            lock_until: if delay > 0 {
                now.saturating_add(delay)
            } else {
                0
            },
            lock_boot: boot,
        }
    }

    /// How long input stays refused; never more than the longest step, whatever was stored.
    pub fn remaining_ms(s: State, now: i64) -> i64 {
        s.lock_until.saturating_sub(now).clamp(0, MAX_MS)
    }

    /// The state after one more wrong (or not yet verified) try.
    pub fn failed(s: State, now: i64, boot: i64) -> State {
        let fails = s.fails.saturating_add(1);
        let delay = delay_after(fails);
        State {
            fails,
            lock_until: if delay > 0 {
                now.saturating_add(delay)
            } else {
                0
            },
            lock_boot: boot,
        }
    }
}

/// When the lock asks for the PIN.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    App,
    Streams,
    Selected,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Off, Mode::App, Mode::Streams, Mode::Selected];

    pub fn from_str_lossy(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "app" => Self::App,
            "streams" => Self::Streams,
            "selected" => Self::Selected,
            _ => Self::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::App => "app",
            Self::Streams => "streams",
            Self::Selected => "selected",
        }
    }
}

/// The SHA-256 of a hand-over's token, which only the started process's input carries, and when
/// it was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandOver {
    pub hash: String,
    pub at: i64,
    pub boot: i64,
}

/// What is in `pin_lock.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub hash: Option<String>,
    pub length: usize,
    pub mode: Mode,
    /// The PCs Selected mode asks for, by their unique id.
    pub pcs: Vec<String>,
    pub grace_min: u32,
    pub fails: u32,
    pub lock_until: i64,
    pub lock_boot: i64,
    pub active_at: i64,
    pub active_boot: i64,
    /// An open lock handed to the second screen's process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hand_over: Option<HandOver>,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            hash: None,
            length: MAX_DIGITS,
            mode: Mode::Off,
            pcs: Vec::new(),
            grace_min: DEFAULT_GRACE_MINUTES,
            fails: 0,
            lock_until: 0,
            lock_boot: 0,
            active_at: 0,
            active_boot: -1,
            hand_over: None,
        }
    }
}

impl Saved {
    /// What the lock holds while its file cannot be used: the app locked, no PIN that matches.
    /// Kept in memory only, never saved.
    pub fn closed() -> Self {
        Saved {
            hash: Some(String::new()),
            mode: Mode::App,
            grace_min: 0,
            ..Saved::default()
        }
    }
}

/// The lock's settings as the settings page shows them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub has_pin: bool,
    pub mode: Mode,
    pub pcs: Vec<String>,
    pub grace_minutes: u32,
}

/// What a try came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// `locked_for_ms` is set when this try started a lockout.
    Wrong {
        locked_for_ms: i64,
    },
    Locked {
        locked_for_ms: i64,
    },
    /// The lock's file cannot be used; the try was not counted.
    Unavailable,
}

/// Why the lock's file cannot be used. The lock stays closed meanwhile, and the file is read
/// again every [`RETRY_MS`], so deleting it, or whatever held it letting go, opens the lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Reading it failed in a way that may pass (another program has it open, ...).
    Unreadable(String),
    /// It is there but not a lock this module wrote; only deleting it removes the PIN lock.
    Damaged(String),
}

pub fn grace_of(minutes: u32) -> u32 {
    if GRACE_CHOICES.contains(&minutes) {
        minutes
    } else {
        DEFAULT_GRACE_MINUTES
    }
}

/// Abstract PIN store backend so UI crates can stay free of `ring`, filesystem I/O and SDKs.
pub trait PinStoreBackend: Send + Sync {
    fn path(&self) -> &Path;
    fn problem(&self) -> Option<Problem>;
    fn has_pin(&self) -> bool;
    fn view(&self) -> View;
    fn pin_length(&self) -> usize;
    fn mode(&self) -> Mode;
    fn set_mode(&self, mode: Mode);
    fn set_pcs(&self, pcs: Vec<String>);
    fn grace_minutes(&self) -> u32;
    fn set_grace_minutes(&self, minutes: u32);
    fn set_pin(&self, pin: &[u8]) -> Result<(), String>;
    fn remove_pin(&self);
    fn locked_for_ms(&self) -> i64;
    fn try_pin(&self, pin: &[u8]) -> Outcome;
    fn set_active(&self, elapsed: i64, boot: i64);
    fn active_at(&self) -> i64;
    fn active_boot(&self) -> i64;
    fn is_this_boot(&self, stored: i64) -> bool;
    fn hand_over(&self) -> Result<Option<String>, String>;
    fn end_hand_over(&self);
    fn handed_over(&self, token: &str) -> bool;
    fn elapsed(&self) -> i64;
    fn boot(&self) -> i64;
    fn flush(&self);
}

/// The key a stream start on a PC is asked and granted under: its unique id, or, for a PC that
/// gave none, Nightjar's own id for it, so the right PIN still lets that start through.
pub fn stream_key(unique_id: &str, host_id: u64) -> String {
    if unique_id.is_empty() {
        format!("host:{host_id}")
    } else {
        unique_id.to_owned()
    }
}

pub fn app_needed(pin_set: bool, mode: Mode, unlocked: bool) -> bool {
    pin_set && mode == Mode::App && !unlocked
}

pub fn stream_needed(
    pin_set: bool,
    mode: Mode,
    pcs: &[String],
    key: &str,
    unlocked: bool,
    grant_valid: bool,
) -> bool {
    if app_needed(pin_set, mode, unlocked) {
        return true;
    }
    if !pin_set {
        return false;
    }
    let asks = mode == Mode::Streams || (mode == Mode::Selected && contains(pcs, key));
    asks && !grant_valid
}

pub fn contains(pcs: &[String], uuid: &str) -> bool {
    !uuid.is_empty() && pcs.iter().any(|pc| pc.eq_ignore_ascii_case(uuid))
}

/// Whether the app was left unlocked recently enough on this boot.
pub fn within_grace(same_boot: bool, left_at: i64, now: i64, grace_minutes: u32) -> bool {
    let away = now.saturating_sub(left_at);
    same_boot && grace_minutes > 0 && away >= 0 && away < i64::from(grace_minutes) * 60_000
}

/// Decides when the PIN is asked for: Android's PinGate without its activity bookkeeping.
#[derive(Clone, Debug, Default)]
pub struct PinGate {
    unlocked_app: bool,
    grant: Option<(String, i64)>,
    /// When the window went out of sight, None while it is in view.
    left_at: Option<i64>,
    /// The second screen's process, handed the first's open lock.
    handed_over: bool,
}

impl PinGate {
    /// An empty locked gate (used before `install` or in tests).
    pub const fn empty() -> Self {
        Self {
            unlocked_app: false,
            grant: None,
            left_at: None,
            handed_over: false,
        }
    }

    /// After a restart the app counts as unlocked if it was left unlocked a moment ago.
    pub fn new(store: &dyn PinStoreBackend) -> Self {
        let unlocked_app = store.has_pin()
            && store.mode() == Mode::App
            && within_grace(
                store.is_this_boot(store.active_boot()),
                store.active_at(),
                store.elapsed(),
                store.grace_minutes(),
            );
        Self {
            unlocked_app,
            grant: None,
            left_at: None,
            handed_over: false,
        }
    }

    /// The app is locked and nothing has unlocked it.
    pub fn needs_pin_for_app(&self, store: &dyn PinStoreBackend) -> bool {
        app_needed(store.has_pin(), store.mode(), self.unlocked_app)
    }

    /// Whether starting a stream under `key` ([`stream_key`]) must first go through the PIN
    /// screen. A yes answer from the PIN screen ([`Self::grant`]) is used up by the start it was
    /// given for.
    pub fn needs_pin_for_stream(&mut self, store: &dyn PinStoreBackend, key: &str) -> bool {
        let needed = self.locks_stream(store, key);
        if !needed && self.granted(store.elapsed(), key) {
            self.grant = None;
        }
        needed
    }

    /// The same answer without using up a grant: Continue here's press asks this, and the stream
    /// start that follows the press uses the grant.
    pub fn locks_stream(&self, store: &dyn PinStoreBackend, key: &str) -> bool {
        if self.handed_over {
            return false;
        }
        let view = store.view();
        stream_needed(
            view.has_pin,
            view.mode,
            &view.pcs,
            key,
            self.unlocked_app,
            self.granted(store.elapsed(), key),
        )
    }

    fn granted(&self, now: i64, key: &str) -> bool {
        self.grant.as_ref().is_some_and(|(pc, at)| {
            !key.is_empty()
                && pc.eq_ignore_ascii_case(key)
                && (0..GRANT_MS).contains(&now.saturating_sub(*at))
        })
    }

    /// The PIN was entered for starting a stream under `key`.
    pub fn grant(&mut self, store: &dyn PinStoreBackend, key: &str) {
        self.grant_at(key, store.elapsed());
    }

    /// The PIN was entered for starting a stream under `key` at monotonic time `now_ms`.
    pub fn grant_at(&mut self, key: &str, now_ms: i64) {
        self.grant = Some((key.to_owned(), now_ms));
    }

    /// The start a grant was left for will not come: it must not open the next start.
    pub fn forget_grant(&mut self) {
        self.grant = None;
    }

    /// The PIN was entered (or has just been set): the app stays open until it is left for the grace time.
    pub fn unlock_app(&mut self) {
        self.unlocked_app = true;
        self.left_at = None;
    }

    /// This is the second screen's process, which the first's open lock was handed to: it streams
    /// the first's game and ends with it, and asks for no PIN.
    pub fn hand_over(&mut self) {
        self.handed_over = true;
        self.unlocked_app = true;
        self.left_at = None;
    }

    /// The window went out of sight (minimized or hidden), or the app is closing.
    pub fn left(&mut self, store: &dyn PinStoreBackend) {
        if self.left_at.is_some() {
            return;
        }
        let now = store.elapsed();
        self.left_at = Some(now);
        // The second screen's process writes nothing: its lock was the first's, which keeps the file.
        if self.unlocked_app && !self.handed_over && store.has_pin() {
            store.set_active(now, store.boot());
        }
    }

    /// The window is in view again: the app locks when it was away longer than the grace time.
    pub fn returned(&mut self, store: &dyn PinStoreBackend) {
        let Some(left_at) = self.left_at.take() else {
            return;
        };
        if self.unlocked_app
            && !self.handed_over
            && store.mode() == Mode::App
            && !within_grace(true, left_at, store.elapsed(), store.grace_minutes())
        {
            self.unlocked_app = false;
        }
    }
}

#[cfg(test)]
mod tests;
