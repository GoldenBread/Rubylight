//! RTX HDR's tuning from the NVIDIA driver profile of the game in front. On Linux no driver profile is read yet: every game has no tuning, so RTX HDR's values stay the app's or the device's.
use std::collections::BTreeMap;

type Tuning = BTreeMap<String, i64>;

/// The driver profile values of the game in front, by setting name.
pub struct Profiles {
    tuning: Tuning,
}
impl Profiles {
    pub fn new() -> Self {
        Self {
            tuning: Tuning::new(),
        }
    }
    /// The tuning of the game whose executable is `key`. Always empty: no driver profile
    /// is read on Linux.
    pub fn poll(&mut self, _key: Option<&str>) -> &Tuning {
        &self.tuning
    }
}
impl Default for Profiles {
    fn default() -> Self {
        Self::new()
    }
}
