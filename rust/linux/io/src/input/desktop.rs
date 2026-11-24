//! The desktop that receives input, and display work scoped to it. Linux has one desktop per
//! session and no secure desktop a thread must move to: the queries report no secure desktop,
//! and scoped work runs where it is called.

/// Move the calling thread to the desktop that receives input. Linux has no such desktop to
/// move to, so the thread stays where it is and this reports false.
pub fn follow_input_desktop() -> bool {
    false
}

/// The name of the desktop that receives input. None on Linux, which names none.
pub fn input_desktop_name() -> Option<String> {
    None
}

/// Whether the PC shows a lock screen, a sign-in screen or an elevation prompt on a desktop
/// of its own. Never on Linux, which has no secure desktop the host could tell from the
/// user's.
pub fn secure_desktop_shown() -> bool {
    false
}

/// Whether the system refused display configuration to the calling thread's desktop. Never
/// on Linux: no error there means that.
pub fn desktop_refused(error: &anyhow::Error) -> bool {
    let _ = error;
    false
}

/// For threads that keep a display's layout: follow the input desktop to the lock screen and
/// back. Does nothing on Linux, where a thread has no desktop to follow.
pub fn keep_on_input_desktop() {}

/// Run display configuration where the system allows it. On Linux that is the calling
/// thread, so `work` runs there and its result is returned.
pub fn on_input_desktop<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    work()
}
