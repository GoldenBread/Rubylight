//! The display restore hotkey on Linux. No global hotkey can be registered yet, so
//! registering one fails and nothing ever reports a press.

use anyhow::Result;

/// A registered hotkey, released when dropped. None exists on Linux yet:
/// [`Hotkey::register`] always fails.
pub struct Hotkey {
    _registered: (),
}

impl Hotkey {
    /// Register `key` (a virtual-key code) with `modifiers` (MOD_* bits) and call `pressed`
    /// on each press until the hotkey is dropped. Fails on Linux, where no global hotkey can
    /// be registered yet; `pressed` is never called.
    pub fn register(key: u32, modifiers: u32, pressed: impl Fn() + Send + 'static) -> Result<Self> {
        let _ = (key, modifiers, pressed);
        Err(crate::unsupported("A global hotkey"))
    }
}
