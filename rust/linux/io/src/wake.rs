//! Wake-on-LAN readiness, idle time and keeping the PC awake on Linux. Reading how long the PC slept and running a child with a deadline work; adapters, NIC wake settings, power capabilities, sleeping the PC and waking the display are not available yet, so the host acts as if the PC had no adapter to arm and no idle time to read.

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

/// Seconds since the last input in this session. Linux does not read it yet.
pub fn idle_seconds() -> Result<u32> {
    Err(crate::unsupported("Reading the idle time"))
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

/// Puts the PC to sleep and returns after it resumes.
pub fn suspend() -> Result<()> {
    Err(crate::unsupported("Sleeping the PC"))
}

/// A request to keep the PC awake, held until dropped. It keeps nothing awake yet.
pub struct PowerRequest {
    _reason: (),
}

impl PowerRequest {
    pub fn system_required(reason: &str) -> Result<Self> {
        let _ = reason;
        Ok(Self { _reason: () })
    }
}

/// Seconds without input after which a launch wakes the console display.
pub const WAKE_IDLE_SECONDS: u32 = 60;

/// Whether a launch should wake the display: the user has been idle long enough for it to be
/// switched off.
pub fn should_wake(idle_seconds: u32) -> bool {
    idle_seconds >= WAKE_IDLE_SECONDS
}

/// Wake an off console display. Linux does not wake it yet.
pub fn wake_console(reason: &str) -> Result<()> {
    let _ = reason;
    Err(crate::unsupported("Waking the display"))
}

/// Wake the console display only when [`should_wake`] says so. Returns whether a wake was
/// sent: never, as the idle time is not read.
pub fn wake_console_if_idle(reason: &str) -> bool {
    let _ = reason;
    false
}
