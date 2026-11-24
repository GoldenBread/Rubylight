//! Whether this host may update itself: only an installed Windows service does. On Linux the
//! package manager (or whoever built the host) owns its files, and the releases on GitHub carry
//! only the Windows installer, so it never checks for a release either.
use crate::state::Shared;

/// Why this host never checks for or installs a release itself, when it does not. Shown in
/// the console and logged once.
pub fn disabled_reason() -> Option<&'static str> {
    // The schedule's tests run on every OS and exercise the checks themselves.
    cfg!(all(
        target_os = "linux",
        not(any(test, feature = "test-support"))
    ))
    .then_some("Rubylight on Linux is updated with your package manager, not by itself")
}

/// `Some` while this host checks for releases; `None` once [`disabled_reason`] says it never
/// does, which is logged the first time.
pub(super) fn checks_on() -> Option<()> {
    let Some(reason) = disabled_reason() else {
        return Some(());
    };
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| tracing::info!(reason, "update checks are off"));
    None
}

/// Why an update cannot be installed here.
pub(super) fn refusal() -> &'static str {
    disabled_reason().unwrap_or("Install Rubylight as a Windows service to use in-app updates")
}

/// The host installs its own updates: an installed Windows service.
pub fn install_supported(h: &Shared) -> bool {
    supported(h)
}

pub(super) fn supported(h: &Shared) -> bool {
    let profile = butterpollo_session::paths::installed_profile();
    butterpollo_win_io::process::is_system()
        && std::fs::canonicalize(&h.directory)
            .ok()
            .zip(std::fs::canonicalize(profile).ok())
            .is_some_and(|(a, b)| a == b)
}
