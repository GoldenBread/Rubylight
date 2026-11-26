//! What the host's lifecycle does differently off Windows: a Linux host's recovery from an
//! interrupted stream at startup, SIGTERM as a stop request, and a restart under systemd.

/// Undoes what an interrupted Linux stream left behind: the video encoder's raised clock goes
/// back to automatic, and a virtual display goes with the monitors it changed restored.
#[cfg(target_os = "linux")]
pub(super) fn recover_interrupted_stream() {
    butterpollo_win_graphics::encoder_clock::recover();
    if let Err(error) = butterpollo_win_display::display::recover_after_restart() {
        tracing::error!(error = %format!("{error:#}"), "the monitors an interrupted stream's virtual display changed were not restored");
    }
}

/// Resolves when systemd (or a shell) asks the host to stop with SIGTERM; never on Windows.
pub(super) async fn terminated() {
    #[cfg(unix)]
    if let Ok(mut stream) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        stream.recv().await;
        return;
    }
    std::future::pending::<()>().await
}

/// Whether the host is a systemd unit's own process: systemd sets SYSTEMD_EXEC_PID to the pid
/// it started. A program a unit's process started (a terminal's shell) inherits the variable
/// with another pid, and INVOCATION_ID alike, so neither is enough on its own.
pub(super) fn under_systemd() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("SYSTEMD_EXEC_PID")
            .is_ok_and(|pid| pid.trim().parse() == Ok(std::process::id()))
}
