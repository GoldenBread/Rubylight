use core::time::Duration;

/// Get the amount of time since the Unix epoch, for use with
/// [`HostSettings::time`](`crate::HostSettings::time`).
///
/// # Panics
///
/// Panics if the resulting time would somehow be negative.
#[must_use]
pub fn time_since_epoch() -> Duration {
    #[cfg(not(target_arch = "wasm32"))]
    {
        #[cfg(not(feature = "std"))]
        {
            panic!("ENet host must be provided a custom time function in no_std environment.");
        }
        #[cfg(feature = "std")]
        {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Time went backwards")
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        Duration::from_millis(js_sys::Date::now() as u64)
    }
}

/// Get the amount of time since the first call in this process, from the monotonic clock, for use
/// with [`HostSettings::time`](`crate::HostSettings::time`). The default of
/// [`HostSettings`](`crate::HostSettings`) (Nightjar/Rubylight change: upstream defaults to
/// [`time_since_epoch`]). The wall clock steps with an NTP adjustment or a manual change, and a
/// step backward makes every reliable command's timeout look overdue; this clock only moves
/// forward, and starts near zero so ENet's wrapping 32 bit milliseconds have their full range.
///
/// Where no monotonic clock is available (wasm, `no_std`) this is [`time_since_epoch`].
#[must_use]
pub fn monotonic_time() -> Duration {
    #[cfg(all(feature = "std", not(target_arch = "wasm32")))]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed()
    }
    #[cfg(not(all(feature = "std", not(target_arch = "wasm32"))))]
    {
        time_since_epoch()
    }
}
