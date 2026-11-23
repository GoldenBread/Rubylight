//! Unified PIN lock persistence, PBKDF2-HMAC-SHA256 hashing and gate policy shared by
//! Nightjar Desktop and Nightjar Android (`nightjar-android::pin`).

pub mod hasher;
pub mod store;

pub use hasher::RingPinCrypto;
pub use nightjar_policy::pin::{
    Clock, DEFAULT_GRACE_MINUTES, FILE, GRACE_CHOICES, GRANT_MS, HAND_OVER_MS, HandOver,
    MAX_DIGITS, MIN_DIGITS, Mode, Outcome, PinCrypto, PinGate, PinStoreBackend, Problem, RETRY_MS,
    Saved, View, app_needed, contains, grace_of, stream_key, stream_needed, throttle, within_grace,
};
pub use store::{PinStore, import_legacy_json, write_private};

#[cfg(test)]
mod tests;
