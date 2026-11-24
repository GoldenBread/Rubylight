//! The scheduling priority guard of capture, encode and input threads, which does nothing on Linux.

/// The scheduling priority of a capture, encode or input thread, put back when dropped.
/// Linux threads keep their priority: the value holds nothing.
pub struct Priority {
    _private: (),
}
/// Ends what the value holds, at the point the host drops it. It raised no priority on Linux, so there is nothing to lower.
impl Drop for Priority {
    fn drop(&mut self) {}
}

impl Default for Priority {
    fn default() -> Self {
        Self::new()
    }
}

impl Priority {
    /// For a thread that lives as long as its stream.
    pub fn new() -> Self {
        Self { _private: () }
    }
    /// For a borrowed (pool) thread, which gets its own priority back.
    pub fn scoped() -> Self {
        Self::new()
    }
    /// For an input thread.
    pub fn input() -> Self {
        Self::new()
    }
}
