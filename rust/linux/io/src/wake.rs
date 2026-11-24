//! Wake-on-LAN readiness, idle time and keeping the PC awake on Linux. Keeping it awake (a logind inhibitor), sleeping it, the idle time and waking the display work (`display`), as does reading how long the PC slept and running a child with a deadline; adapters, NIC wake settings and power capabilities are not available yet, so the host acts as if the PC had no adapter to arm.

mod display;

use anyhow::{Context, Result};
use butterpollo_session::wake::{Adapter, ApplyReport, FixPlan, NicReport, SystemFacts};
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};

/// How long reading the NIC wake settings may take.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// How long applying them may take before the caller stops waiting.
pub const APPLY_TIMEOUT: Duration = Duration::from_secs(180);
/// How long `--apply-wake-settings` may take: a read and an apply, each bounded above, and
/// some slack. Whoever starts it waits at least this long.
pub const SETTINGS_TIMEOUT: Duration =
    Duration::from_secs(READ_TIMEOUT.as_secs() + APPLY_TIMEOUT.as_secs() + 20);

/// Every adapter the PC has with the facts the MAC policy needs: none are listed yet.
pub fn adapters() -> Result<Vec<Adapter>> {
    Ok(Vec::new())
}

/// The PC's own address on its default route; None, as no route is read yet.
pub fn default_route_source() -> Result<Option<IpAddr>> {
    Ok(None)
}

/// The wake settings of every physical adapter: none are read yet.
pub fn read_nics() -> Result<NicReport> {
    Ok(NicReport::default())
}

/// Carries out `plan`. Linux changes no adapter yet, so nothing changed.
pub fn apply_nics(plan: &FixPlan) -> Result<ApplyReport> {
    let _ = plan;
    Ok(ApplyReport::default())
}

/// Run `command` with `input` on its stdin; its exit status, stdout and stderr. It is killed if
/// it hasn't finished within `timeout`; its own children are not.
pub fn output_within(
    command: &mut std::process::Command,
    input: &[u8],
    timeout: Duration,
) -> Result<(std::process::ExitStatus, String, String)> {
    use std::{
        io::{Read, Write},
        process::Stdio,
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", command.get_program().to_string_lossy()))?;
    let read = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = read(child.stdout.take().map(|s| Box::new(s) as _));
    let stderr = read(child.stderr.take().map(|s| Box::new(s) as _));
    // Dropped after writing, so the child sees the end of its input.
    let written = child
        .stdin
        .take()
        .context("the child has no stdin")?
        .write_all(input);
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("did not finish within {timeout:?} and was stopped");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    // A child that ended without reading its input closes the pipe; that is not an error.
    if let Err(error) = written
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error).context("writing the child's input");
    }
    Ok((status, stdout, stderr))
}

/// The power capabilities: none are read yet.
pub fn system_facts() -> Result<SystemFacts> {
    Ok(SystemFacts::default())
}

/// The flags every program's request to keep the PC awake adds up to: none are read yet.
pub fn execution_state() -> Result<u32> {
    Ok(0)
}

fn clock_ns(clock: libc::clockid_t) -> Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a live local the call writes once.
    let read = unsafe { libc::clock_gettime(clock, &mut time) };
    anyhow::ensure!(read == 0, "reading the clock failed");
    Ok(time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64)
}

/// Milliseconds the PC has slept since it started: the boot-time clock counts sleep, the
/// monotonic one doesn't.
pub fn slept_ms() -> Result<u64> {
    let boot = clock_ns(libc::CLOCK_BOOTTIME).context("reading the time since boot")?;
    let awake = clock_ns(libc::CLOCK_MONOTONIC).context("reading the time awake")?;
    Ok(boot.saturating_sub(awake) / 1_000_000)
}

/// Seconds since the last input in this session, as the desktop or logind reports it. Fails
/// where none of them knows.
pub fn idle_seconds() -> Result<u32> {
    display::idle_seconds()
}

/// Set in `--idle-seconds`' exit code when it failed; the low 16 bits carry the error.
pub const IDLE_FAILED: u32 = 0x8000_0000;
/// Set in `--idle-seconds`' exit code when it read the time; the low 20 bits carry the
/// seconds. Any other code is a failure, never idle time.
pub const IDLE_READ: u32 = 0x4000_0000;

/// `--idle-seconds`' exit code: [`IDLE_READ`] with [`idle_seconds`], or [`IDLE_FAILED`] with
/// the error (1 when there is none).
pub fn idle_exit_code() -> u32 {
    match idle_seconds() {
        Ok(seconds) => IDLE_READ | seconds,
        Err(_) => IDLE_FAILED | 1,
    }
}

/// Puts the PC to sleep and returns after it resumes (logind's `Suspend`).
pub fn suspend() -> Result<()> {
    crate::power::suspend()
}

/// A request to keep the PC awake, held until dropped: a systemd-logind inhibitor for sleep
/// and idle actions, which `systemd-inhibit --list` shows with its reason.
pub struct PowerRequest {
    _inhibitor: zbus::zvariant::OwnedFd,
}

impl PowerRequest {
    pub fn system_required(reason: &str) -> Result<Self> {
        let inhibitor = crate::power::logind::inhibit("sleep:idle", "Rubylight", reason)?;
        Ok(Self {
            _inhibitor: inhibitor,
        })
    }
}

/// Seconds without input after which a launch wakes the console display.
pub const WAKE_IDLE_SECONDS: u32 = 60;

/// Whether a launch should wake the display: the user has been idle long enough for it to be
/// switched off.
pub fn should_wake(idle_seconds: u32) -> bool {
    idle_seconds >= WAKE_IDLE_SECONDS
}

/// Wake an off console display: simulated user activity on the desktop, nothing for a headless
/// gamescope session.
pub fn wake_console(reason: &str) -> Result<()> {
    display::wake_console(reason)
}

/// Wake the console display only when [`should_wake`] says so. Returns whether a wake was
/// sent; false as well where the idle time cannot be read.
pub fn wake_console_if_idle(reason: &str) -> bool {
    let idle = idle_seconds().unwrap_or(0);
    if !should_wake(idle) {
        return false;
    }
    match wake_console(reason) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(reason, error = %format!("{error:#}"), "could not wake the console display");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inhibitor shows in logind's list while held and is gone after.
    #[test]
    #[ignore = "talks to systemd-logind"]
    fn a_power_request_is_an_inhibitor() {
        let listed = || {
            let out = std::process::Command::new("systemd-inhibit")
                .arg("--list")
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).contains("rubylight-test-reason")
        };
        let request = PowerRequest::system_required("rubylight-test-reason").unwrap();
        assert!(listed());
        drop(request);
        assert!(!listed());
    }

    #[test]
    fn the_display_wakes_after_a_minute_idle() {
        assert!(!should_wake(WAKE_IDLE_SECONDS - 1));
        assert!(should_wake(WAKE_IDLE_SECONDS));
    }
}
