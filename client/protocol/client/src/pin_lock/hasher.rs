//! Salted PBKDF2-HMAC-SHA256 PIN hashing, calibration and constant-time verification
//! matching Android's `v1$iterations$salt$key` format.

use std::num::NonZeroU32;
use std::time::Instant;

use nightjar_policy::pin::PinCrypto;
use ring::digest::{SHA256, digest};
use ring::pbkdf2;
use ring::rand::{SecureRandom, SystemRandom};

use crate::http::der::{base64_decode, base64_encode};

pub const KEY_BYTES: usize = 32;
pub const SALT_BYTES: usize = 16;
pub const TARGET_MS: u64 = 150;
pub const MIN_ITERATIONS: u32 = 20_000;
pub const MAX_ITERATIONS: u32 = 600_000;
pub const PROBE_ITERATIONS: u32 = 10_000;
pub const MAX_STORED_ITERATIONS: u32 = 5_000_000;
pub const VERSION: &str = "v1";
const ALGORITHM: pbkdf2::Algorithm = pbkdf2::PBKDF2_HMAC_SHA256;

/// Derives `key_bytes` of PBKDF2-HMAC-SHA256 from `pin` and `salt`.
pub fn pbkdf2(
    pin: &[u8],
    salt: &[u8],
    iterations: u32,
    key_bytes: usize,
) -> Result<Vec<u8>, String> {
    if pin.is_empty() {
        return Err("empty PIN".into());
    }
    let iterations = NonZeroU32::new(iterations).ok_or("no iterations")?;
    let mut out = vec![0u8; key_bytes];
    if !out.is_empty() {
        pbkdf2::derive(ALGORITHM, iterations, salt, pin, &mut out);
    }
    Ok(out)
}

/// Strict RFC 4648 base64 decoding (no whitespace, valid length/padding).
fn strict_base64_decode(text: &str) -> Option<Vec<u8>> {
    if text.is_empty()
        || !text.len().is_multiple_of(4)
        || text
            .bytes()
            .any(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
    {
        return None;
    }
    base64_decode(text).ok()
}

/// Creates a salted `v1$iterations$salt$key` record for `pin`.
pub fn create(pin: &[u8], iterations: u32) -> Result<String, String> {
    if pin.is_empty() {
        return Err("empty PIN".into());
    }
    let iterations = NonZeroU32::new(iterations).ok_or("no iterations")?;
    let mut salt = [0u8; SALT_BYTES];
    SystemRandom::new()
        .fill(&mut salt)
        .map_err(|_| "the system's random source failed".to_owned())?;
    let mut key = [0u8; KEY_BYTES];
    pbkdf2::derive(ALGORITHM, iterations, &salt, pin, &mut key);
    Ok(format!(
        "{VERSION}${iterations}${}${}",
        base64_encode(&salt),
        base64_encode(&key)
    ))
}

/// Verifies `pin` against a `v1$iterations$salt$key` record in constant time.
pub fn verify(stored: &str, pin: &[u8]) -> bool {
    if pin.is_empty() {
        return false;
    }
    let parts: Vec<&str> = stored.split('$').collect();
    let [version, iterations, salt, key] = parts[..] else {
        return false;
    };
    if version != VERSION {
        return false;
    }
    let Some(iterations) = iterations
        .parse::<u32>()
        .ok()
        .filter(|i| *i <= MAX_STORED_ITERATIONS)
        .and_then(NonZeroU32::new)
    else {
        return false;
    };
    let (Some(salt), Some(key)) = (strict_base64_decode(salt), strict_base64_decode(key)) else {
        return false;
    };
    if salt.is_empty() || key.is_empty() {
        return false;
    }
    pbkdf2::verify(ALGORITHM, iterations, &salt, pin, &key).is_ok()
}

/// Checks whether `stored` is a well-formed `v1$iterations$salt$key` record.
pub fn well_formed(stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    let [version, iterations, salt, key] = parts[..] else {
        return false;
    };
    version == VERSION
        && iterations
            .parse::<u32>()
            .is_ok_and(|i| (1..=MAX_STORED_ITERATIONS).contains(&i))
        && strict_base64_decode(salt).is_some_and(|s| !s.is_empty())
        && strict_base64_decode(key).is_some_and(|k| !k.is_empty())
}

/// Measures a 10,000-iteration probe and returns an iteration count targeting ~150 ms.
pub fn calibrate() -> u32 {
    let probe = b"1234";
    let salt = [0u8; SALT_BYTES];
    let mut out = [0u8; KEY_BYTES];
    pbkdf2::derive(
        ALGORITHM,
        NonZeroU32::new(1_000).unwrap(),
        &salt,
        probe,
        &mut out,
    );
    let start = Instant::now();
    pbkdf2::derive(
        ALGORITHM,
        NonZeroU32::new(PROBE_ITERATIONS).unwrap(),
        &salt,
        probe,
        &mut out,
    );
    scale(start.elapsed().as_nanos() as u64)
}

/// Scales a probe duration in nanoseconds to the target iteration count within bounds.
pub fn scale(probe_nanos: u64) -> u32 {
    let per_iteration = probe_nanos.max(1) as f64 / f64::from(PROBE_ITERATIONS);
    let wanted = TARGET_MS as f64 * 1_000_000.0 / per_iteration;
    wanted.clamp(f64::from(MIN_ITERATIONS), f64::from(MAX_ITERATIONS)) as u32
}

/// Constant-time comparison of two byte slices.
pub fn constant_time_equals(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

/// [`PinCrypto`] backed by `ring` PBKDF2-HMAC-SHA256, CSPRNG and SHA-256.
pub struct RingPinCrypto;

impl PinCrypto for RingPinCrypto {
    fn create(&self, pin: &[u8], iterations: u32) -> Result<String, String> {
        create(pin, iterations)
    }

    fn verify(&self, stored: &str, pin: &[u8]) -> bool {
        verify(stored, pin)
    }

    fn well_formed(&self, stored: &str) -> bool {
        well_formed(stored)
    }

    fn calibrate(&self) -> u32 {
        calibrate()
    }

    fn random_token(&self) -> Result<String, String> {
        let mut bytes = [0u8; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| "the system's random source failed".to_owned())?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    fn token_hash(&self, token: &str) -> String {
        base64_encode(digest(&SHA256, token.as_bytes()).as_ref())
    }
}
