//! What this PC's clock shows at a moment in time, from the C library's time zone rules.

/// The wall-clock time on this PC at `unix` (Unix seconds), as Unix seconds as if that clock
/// were UTC, by the time zone's rule on that date: the opposite of
/// [`crate::playnite::local_to_utc`]. `None` when the moment cannot be converted.
pub fn utc_to_local(unix: u64) -> Option<i64> {
    let moment = libc::time_t::try_from(unix).ok()?;
    // SAFETY: an all-zero `tm` is a valid value (its zone name pointer, if any, is null).
    let mut broken: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to locals that outlive the call, which writes only `broken`.
    let converted = unsafe { libc::localtime_r(&moment, &mut broken) };
    if converted.is_null() {
        return None;
    }
    i64::from(moment).checked_add(i64::from(broken.tm_gmtoff))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_clock_is_within_a_day_of_utc() {
        // Noon UTC on 2026-01-15 and 2026-07-15: winter and summer time.
        for unix in [1_768_478_400_u64, 1_784_116_800] {
            let local = utc_to_local(unix).unwrap();
            assert!((local - unix as i64).abs() <= 14 * 3600, "{unix}: {local}");
        }
        assert_eq!(utc_to_local(u64::MAX), None);
    }
}
