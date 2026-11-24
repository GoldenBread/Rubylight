//! The HDR colour profile a stream's display gets, on Linux.
//!
//! Linux has no backend for per-display colour profiles yet: no profile is installed where
//! the host could find it, no display has a calibration, and a profile lease cannot be taken.
use anyhow::Result;

const PROFILES: &str = "An HDR colour profile";

/// The peak brightness, in nits, the calibration profile `selection` records. Fails on
/// Linux: the host has no installed profiles to read it from.
pub fn peak_luminance(_selection: &str) -> Result<Option<u32>> {
    Err(crate::unsupported(PROFILES))
}

/// An HDR calibration profile of a physical display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calibration {
    /// The physical display it calibrates.
    pub display: String,
    pub profile: String,
    pub peak: u32,
}

/// Runs the calibration lookup as the signed-in user.
type SessionUser = fn(
    Box<dyn FnOnce() -> Result<Option<Calibration>>>,
) -> std::io::Result<Result<Option<Calibration>>>;

/// The calibration of the physical HDR display a virtual display stands in for. None on
/// Linux: no display is listed, so none has one, and `as_session_user` is not called.
pub fn calibration_with(_as_session_user: SessionUser) -> Result<Option<Calibration>> {
    Ok(None)
}

/// A colour profile applied to a display for as long as streams hold it. None can be applied
/// on Linux yet.
pub struct Lease(());

impl Lease {
    /// Apply the profile `selection` to the display `output`. Fails on Linux.
    pub fn acquire(_output: &str, _selection: &str) -> Result<Self> {
        Err(crate::unsupported(PROFILES))
    }
    /// Make this lease hold `selection` on `output` instead. Fails on Linux.
    pub fn switch(&mut self, _output: &str, _selection: &str) -> Result<()> {
        Err(crate::unsupported(PROFILES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_profile_is_found_or_applied() {
        assert!(peak_luminance("calibration.icc").is_err());
        assert_eq!(calibration_with(|work| Ok(work())).unwrap(), None);
        assert!(Lease::acquire("DP-1", "calibration.icc").is_err());
    }
}
