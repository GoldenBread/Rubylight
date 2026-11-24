//! The names and exit codes the host shares with a supervising service. A Linux host runs as the signed-in user and has no service yet, so no service is recorded and there is no Nucleus Co-op session of another account to put back.

/// The service's name in a normal installation.
pub const NAME: &str = butterpollo_session::host_service::DEFAULT;

/// The service this PC's Rubylight runs as: the default name, as none is recorded.
pub fn name() -> &'static str {
    NAME
}

/// The exit code with which the host asks its supervisor to start it again at once.
pub const RESTART_EXIT_CODE: u32 = 75;

/// The exit code with which a supervised host leaves after it shut down for the end of its
/// session. The value is the one Windows gives a process it ends with the session, which
/// keeps one code for both systems.
pub const SESSION_ENDED_EXIT_CODE: u32 = 0x4001_0004;

/// Put back the compatibility settings a Nucleus Co-op session left. Linux changes none, so
/// nothing is left to put back and it succeeds.
pub fn restore_nucleus_layers(config: &std::path::Path) -> bool {
    let _ = config;
    true
}
