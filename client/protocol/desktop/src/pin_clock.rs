//! `ring`-backed PIN hashing and OS boot/monotonic clocks for `nightjar-desktop-config::pin_lock`
//! and `nightjar-desktop-ui::pin_ui`.

use std::num::NonZeroU32;
use std::sync::Arc;

use base64::Engine as _;
use ring::pbkdf2;
use ring::rand::{SecureRandom, SystemRandom};

use nightjar_desktop_config::pin_lock::{Clock, PinCrypto};

/// Registers the OS clock and `ring`-backed PIN crypto with `nightjar-desktop-ui::pin_ui`.
pub fn register() {
    nightjar_desktop_ui::pin_ui::register_backend(
        || Arc::new(SystemClock),
        || Arc::new(RingPinCrypto),
    );
}

/// How far two estimates of the boot time may lie apart and still be the same start (Windows,
/// and Linux without a boot id): the wall clock they come from drifts and is set by NTP.
pub const BOOT_TOLERANCE_MS: i64 = 60_000;

/// Two estimates of the boot time (wall clock minus time since boot) belong to the same start
/// when they lie within [`BOOT_TOLERANCE_MS`].
pub fn same_boot_estimate(stored: i64, now: i64) -> bool {
    stored
        .checked_sub(now)
        .and_then(i64::checked_abs)
        .is_some_and(|d| d < BOOT_TOLERANCE_MS)
}

#[cfg(not(windows))]
fn boot_id_number(id: &str) -> i64 {
    let hash = id
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    hash as i64
}

#[cfg(windows)]
fn uptime_ms() -> i64 {
    // SAFETY: no arguments; counts since boot, sleep included.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() as i64 }
}

#[cfg(not(windows))]
#[allow(clippy::unnecessary_cast, reason = "time_t and c_long are 32-bit on 32-bit Linux")]
fn uptime_ms() -> i64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    #[cfg(target_os = "macos")]
    let clock = libc::CLOCK_MONOTONIC_RAW;
    #[cfg(target_os = "linux")]
    let clock = libc::CLOCK_BOOTTIME;
    if unsafe { libc::clock_gettime(clock, &mut t) } != 0 {
        panic!("boot clock unavailable: {}", std::io::Error::last_os_error());
    }
    t.tv_sec as i64 * 1000 + t.tv_nsec as i64 / 1_000_000
}

fn system_boot() -> (i64, bool) {
    static BOOT: std::sync::OnceLock<(i64, bool)> = std::sync::OnceLock::new();
    *BOOT.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            let mut uuid = [0u8; 37];
            let mut len = uuid.len();
            if unsafe {
                libc::sysctlbyname(
                    c"kern.bootsessionuuid".as_ptr(),
                    uuid.as_mut_ptr().cast(),
                    &mut len,
                    std::ptr::null_mut(),
                    0,
                )
            } == 0
                && let Some(id) = std::ffi::CStr::from_bytes_until_nul(&uuid)
                    .ok()
                    .and_then(|s| s.to_str().ok())
                    .filter(|s| !s.is_empty())
            {
                return (boot_id_number(id), true);
            }
        }
        #[cfg(target_os = "linux")]
        match std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
            Ok(id) if !id.trim().is_empty() => return (boot_id_number(id.trim()), true),
            Ok(_) => eprintln!("nightjar: the kernel's boot id is empty; the PIN lock estimates the boot time instead"),
            Err(e) => eprintln!("nightjar: the kernel's boot id is unreadable ({e}); the PIN lock estimates the boot time instead"),
        }
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        (wall - uptime_ms(), false)
    })
}

/// Real OS uptime and boot-id clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn elapsed(&self) -> i64 {
        uptime_ms()
    }

    fn boot(&self) -> i64 {
        system_boot().0
    }

    fn is_this_boot(&self, stored: i64) -> bool {
        match system_boot() {
            (id, true) => stored == id,
            (estimate, false) => same_boot_estimate(stored, estimate),
        }
    }
}

/// Salted PBKDF2-HMAC-SHA256 implementation matching Android's PinHasher format.
pub mod hasher {
    use super::*;

    pub const KEY_BYTES: usize = 32;
    pub const SALT_BYTES: usize = 16;
    pub const TARGET_MS: u64 = 150;
    pub const MIN_ITERATIONS: u32 = 20_000;
    pub const MAX_ITERATIONS: u32 = 600_000;
    const PROBE_ITERATIONS: u32 = 10_000;
    const MAX_STORED_ITERATIONS: u32 = 5_000_000;
    const VERSION: &str = "v1";
    const ALGORITHM: pbkdf2::Algorithm = pbkdf2::PBKDF2_HMAC_SHA256;

    fn derive(pin: &[u8], salt: &[u8], iterations: NonZeroU32, out: &mut [u8]) {
        pbkdf2::derive(ALGORITHM, iterations, salt, pin, out);
    }

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
        derive(pin, &salt, iterations, &mut key);
        let b64 = base64::engine::general_purpose::STANDARD;
        Ok(format!("{VERSION}${iterations}${}${}", b64.encode(salt), b64.encode(key)))
    }

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
        let b64 = base64::engine::general_purpose::STANDARD;
        let (Ok(salt), Ok(key)) = (b64.decode(salt), b64.decode(key)) else {
            return false;
        };
        if salt.is_empty() || key.is_empty() {
            return false;
        }
        pbkdf2::verify(ALGORITHM, iterations, &salt, pin, &key).is_ok()
    }

    pub fn well_formed(stored: &str) -> bool {
        let b64 = base64::engine::general_purpose::STANDARD;
        let parts: Vec<&str> = stored.split('$').collect();
        let [version, iterations, salt, key] = parts[..] else {
            return false;
        };
        version == VERSION
            && iterations
                .parse::<u32>()
                .is_ok_and(|i| (1..=MAX_STORED_ITERATIONS).contains(&i))
            && b64.decode(salt).is_ok_and(|s| !s.is_empty())
            && b64.decode(key).is_ok_and(|k| !k.is_empty())
    }

    pub fn calibrate() -> u32 {
        let probe = b"1234";
        let salt = [0u8; SALT_BYTES];
        let mut out = [0u8; KEY_BYTES];
        derive(probe, &salt, NonZeroU32::new(1_000).unwrap(), &mut out);
        let start = std::time::Instant::now();
        derive(probe, &salt, NonZeroU32::new(PROBE_ITERATIONS).unwrap(), &mut out);
        scale(start.elapsed().as_nanos() as u64)
    }

    pub fn scale(probe_nanos: u64) -> u32 {
        let per_iteration = probe_nanos.max(1) as f64 / f64::from(PROBE_ITERATIONS);
        let wanted = TARGET_MS as f64 * 1_000_000.0 / per_iteration;
        wanted.clamp(f64::from(MIN_ITERATIONS), f64::from(MAX_ITERATIONS)) as u32
    }
}

/// [`PinCrypto`] backed by `ring` PBKDF2-HMAC-SHA256, CSPRNG and SHA-256.
pub struct RingPinCrypto;

impl PinCrypto for RingPinCrypto {
    fn create(&self, pin: &[u8], iterations: u32) -> Result<String, String> {
        hasher::create(pin, iterations)
    }

    fn verify(&self, stored: &str, pin: &[u8]) -> bool {
        hasher::verify(stored, pin)
    }

    fn well_formed(&self, stored: &str) -> bool {
        hasher::well_formed(stored)
    }

    fn calibrate(&self) -> u32 {
        hasher::calibrate()
    }

    fn random_token(&self) -> Result<String, String> {
        let mut bytes = [0u8; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| "the system's random source failed".to_owned())?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    fn token_hash(&self, token: &str) -> String {
        base64::engine::general_purpose::STANDARD
            .encode(ring::digest::digest(&ring::digest::SHA256, token.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_verifies_its_pin_only_and_keeps_androids_format() {
        let record = hasher::create(b"2580", 1_000).unwrap();
        let parts: Vec<&str> = record.split('$').collect();
        assert_eq!(parts.len(), 4, "{record}");
        assert_eq!((parts[0], parts[1]), ("v1", "1000"));
        let b64 = base64::engine::general_purpose::STANDARD;
        assert_eq!(b64.decode(parts[2]).unwrap().len(), hasher::SALT_BYTES);
        assert_eq!(b64.decode(parts[3]).unwrap().len(), hasher::KEY_BYTES);
        assert!(!record.contains("2580"), "the PIN is never stored");
        assert!(hasher::verify(&record, b"2580"));
        for wrong in [&b"2581"[..], b"258", b"25800", b""] {
            assert!(!hasher::verify(&record, wrong), "{wrong:?}");
        }
        assert_ne!(
            record,
            hasher::create(b"2580", 1_000).unwrap(),
            "every PIN gets a fresh salt"
        );
    }

    #[test]
    fn a_record_androids_pin_hasher_wrote_verifies() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut key = [0u8; 32];
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(1000).unwrap(),
            &[0; 16],
            b"1234",
            &mut key,
        );
        let record = format!("v1$1000${}${}", b64.encode([0u8; 16]), b64.encode(key));
        assert!(hasher::verify(&record, b"1234"));
        let mut out = [0u8; 64];
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(1).unwrap(),
            b"salt",
            b"passwd",
            &mut out,
        );
        assert_eq!(&out[..8], &[0x55, 0xac, 0x04, 0x6e, 0x56, 0xe3, 0x08, 0x9f]);
    }

    #[test]
    fn records_that_were_not_written_here_never_verify() {
        let good = hasher::create(b"1234", 1_000).unwrap();
        let (_, rest) = good.split_once('$').unwrap();
        for bad in [
            String::new(),
            format!("v2${rest}"),
            good.replacen("1000", "0", 1),
            good.replacen("1000", "99999999", 1),
            good.replacen("1000", "x", 1),
            "v1$1000$$".into(),
            "v1$1000$!!$!!".into(),
            format!("{good}$extra"),
        ] {
            assert!(!hasher::verify(&bad, b"1234"), "{bad}");
        }
    }

    #[test]
    fn calibration_aims_at_the_target_and_stays_in_range() {
        assert_eq!(hasher::scale(15_000_000), 100_000, "1.5 us an iteration: 150 ms is 100k");
        assert_eq!(hasher::scale(1), hasher::MAX_ITERATIONS);
        assert_eq!(hasher::scale(u64::MAX), hasher::MIN_ITERATIONS);
        let measured = hasher::calibrate();
        assert!((hasher::MIN_ITERATIONS..=hasher::MAX_ITERATIONS).contains(&measured));
    }

    #[test]
    fn a_boot_estimate_survives_clock_drift_but_not_a_restart() {
        let boot = 1_700_000_000_000;
        assert!(same_boot_estimate(boot, boot + 59_000));
        assert!(same_boot_estimate(boot, boot - 59_000));
        assert!(!same_boot_estimate(boot, boot + 10 * 60_000));
        assert!(!same_boot_estimate(i64::MIN, 0) && !same_boot_estimate(-1, boot));
        let clock = SystemClock;
        assert!(clock.is_this_boot(clock.boot()), "this run's own boot");
        assert_eq!(clock.boot(), clock.boot(), "measured once per run");
        #[cfg(not(windows))]
        assert_ne!(boot_id_number("a"), boot_id_number("b"));
    }
}
