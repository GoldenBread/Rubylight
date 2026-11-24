//! The PC's own keyboard and mouse, told apart from what streams inject. Linux has no watcher
//! for them yet, so starting one fails and someone at the PC cannot take the keyboard and
//! mouse back from a guest by using them.
use anyhow::Result;

/// Watches until dropped. None exists on Linux yet: [`watch`] always fails.
pub struct Watch {
    _watching: (),
}
/// Ends what the value holds, at the point the host drops it. It watches nothing on Linux yet, so there is nothing to stop.
impl Drop for Watch {
    fn drop(&mut self) {}
}

/// Calls `on_input` when someone uses the PC's own keyboard or mouse. Fails on Linux, where
/// local input is not watched yet; `on_input` is never called.
pub fn watch(on_input: impl Fn() + Send + 'static) -> Result<Watch> {
    let _ = on_input;
    Err(crate::unsupported(
        "Watching the PC's own keyboard and mouse",
    ))
}
