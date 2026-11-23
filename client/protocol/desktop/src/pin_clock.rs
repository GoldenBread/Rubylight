//! OS boot/monotonic clocks and `nightjar_core::pin_lock` registration for `nightjar-desktop`.

use std::sync::Arc;

use nightjar_core::pin_lock::{Clock, PinStore, RingPinCrypto};

/// Registers the real `nightjar_core::pin_lock::PinStore` backend with `nightjar-desktop-ui::pin_ui`.
pub fn register() {
    nightjar_desktop_ui::pin_ui::register_backend(|dir| {
        Arc::new(PinStore::new(dir, Arc::new(SystemClock), Arc::new(RingPinCrypto)))
    });
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn register_wires_pin_lock_open_to_real_pbkdf2_pin_store() {
        register();
        let dir = std::env::temp_dir().join(format!("nightjar-pin-clock-real-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        {
            let lock = nightjar_desktop_ui::pin_ui::PinLock::open(&dir);
            lock.store.set_pin(b"2468").unwrap();
            lock.store.flush();
            let raw = std::fs::read_to_string(dir.join(nightjar_core::pin_lock::FILE)).unwrap();
            assert!(raw.contains("v1$"), "real PBKDF2 hash stored: {raw}");
            assert!(!raw.contains("stub"), "must not use StubPinStore: {raw}");
            assert!(!raw.contains("2468"), "must not store plaintext PIN: {raw}");
            assert!(matches!(
                lock.store.try_pin(b"1111"),
                nightjar_policy::pin::Outcome::Wrong { .. }
            ));
            assert_eq!(lock.store.try_pin(b"2468"), nightjar_policy::pin::Outcome::Ok);
        }
        {
            let reopened = nightjar_desktop_ui::pin_ui::PinLock::open(&dir);
            assert!(reopened.store.has_pin(), "persists across PinLock::open");
            assert_eq!(reopened.store.try_pin(b"2468"), nightjar_policy::pin::Outcome::Ok);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
