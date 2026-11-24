//! The end of the user's session, heard by the host. Linux does not listen for it yet: the watch starts and never reports an end, so clients learn of one by their timeout.

use anyhow::Result;

/// How the session ends, for the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ending {
    /// The user signs out; otherwise the PC shuts down or restarts.
    pub logoff: bool,
    /// The session ends whatever the programs answer.
    pub critical: bool,
    /// A program is asked to close as well.
    pub close_app: bool,
}

/// Listens for the end of the session until dropped.
pub struct SessionEndWatch {
    _listening: (),
}
/// Ends what the value holds, at the point the host drops it. It listens to nothing on Linux, so there is nothing to stop.
impl Drop for SessionEndWatch {
    fn drop(&mut self) {}
}

impl SessionEndWatch {
    /// Start listening. `on_end` is called once with how the session ends; it is never
    /// called yet.
    pub fn start(on_end: impl FnOnce(Ending) + Send + 'static) -> Result<Self> {
        drop(on_end);
        Ok(Self { _listening: () })
    }
}
