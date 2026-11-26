//! Wake-on-LAN readiness, the consented NIC fix, the wake grace and sleep
//! after idle (`docs/wake-and-audio-design.md`, A.3 to A.5). Everything
//! Windows answers goes through [`PowerSystem`], which tests replace with a
//! fake.
use crate::state::Host;
use anyhow::{Context, Result};
#[cfg(test)]
use butterpollo_session::wake::idle;
use butterpollo_session::wake::{
    self as readiness, Adapter, ApplyReport, FixPlan, NicReport, SystemFacts,
    grace::WakeGrace,
    idle::{IdleSleep, Input, Settings, Snapshot, Status},
};
use butterpollo_win_io::wake::{IDLE_FAILED, IDLE_READ};
use rubylight_native_api::{session::State as Session, wake as api};
#[cfg(test)]
use serde_json::{Value, json};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    net::IpAddr,
    sync::{Arc, Mutex, PoisonError, RwLock},
    time::{Duration, Instant, SystemTime},
};

mod console;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
mod fix;

pub use console::{console_status, diagnostics};
pub use fix::{
    Applied, FIX_DELAY, FixRefusal, apply_plan, apply_settings, apply_settings_exit_code, fix_plan,
    fix_refusal, request_fix,
};
use fix::{FIX_VARIABLE, Fix};

/// How long a reading of the NIC settings stays fresh; a fix or `refresh` reads
/// again.
const NIC_CACHE: Duration = Duration::from_secs(60);
/// How long the `--idle-seconds` helper may take.
const IDLE_HELPER_WAIT: Duration = Duration::from_secs(10);
/// The wake grace's power request, as `powercfg /requests` shows it.
const GRACE_REASON: &str = "A device is connecting to Rubylight after a wake";

/// What the host asks Windows.
pub trait PowerSystem: Send + Sync {
    fn adapters(&self) -> Result<Vec<Adapter>>;
    /// The PC's own address on its default route; `None` without one.
    fn default_route(&self) -> Result<Option<IpAddr>>;
    /// The wake settings of every physical adapter.
    fn read_nics(&self) -> Result<NicReport>;
    fn system_facts(&self) -> Result<SystemFacts>;
    /// `SystemExecutionState`.
    fn execution_state(&self) -> Result<u32>;
    /// How long the PC has slept since it started.
    fn slept(&self) -> Result<Duration>;
    /// Local input in the console session.
    fn input(&self) -> Input;
    /// Sleep (S3); returns after the PC resumes.
    fn sleep(&self) -> Result<()>;
    /// Carries out `plan`: changed adapters restart.
    fn apply_nics(&self, plan: &FixPlan) -> Result<ApplyReport>;
    /// `butterpollo.exe --apply-wake-settings` in a child process, as the
    /// host's own account (SYSTEM under the service), applying exactly `plan`.
    fn run_fix(&self, plan: &FixPlan) -> Result<Applied>;
    /// A system-required power request, held until the result is dropped.
    fn hold_awake(&self, reason: &str) -> Result<Box<dyn Send>>;
}

struct Windows;

impl PowerSystem for Windows {
    fn adapters(&self) -> Result<Vec<Adapter>> {
        butterpollo_win_io::wake::adapters()
    }
    fn default_route(&self) -> Result<Option<IpAddr>> {
        butterpollo_win_io::wake::default_route_source()
    }
    fn read_nics(&self) -> Result<NicReport> {
        butterpollo_win_io::wake::read_nics()
    }
    fn system_facts(&self) -> Result<SystemFacts> {
        butterpollo_win_io::wake::system_facts()
    }
    fn execution_state(&self) -> Result<u32> {
        butterpollo_win_io::wake::execution_state()
    }
    fn slept(&self) -> Result<Duration> {
        butterpollo_win_io::wake::slept_ms().map(Duration::from_millis)
    }
    fn input(&self) -> Input {
        use butterpollo_win_io::process;
        if !process::is_system() {
            // A portable host runs in the user's own session.
            return match butterpollo_win_io::wake::idle_seconds() {
                Ok(seconds) => Input::Idle(Duration::from_secs(seconds.into())),
                Err(error) => Input::Failed(format!("{error:#}")),
            };
        }
        if !process::user_signed_in() {
            return Input::NobodySignedIn;
        }
        match idle_helper() {
            Ok(code) => helper_input(code),
            Err(error) => Input::Failed(format!("{error:#}")),
        }
    }
    fn sleep(&self) -> Result<()> {
        butterpollo_win_io::wake::suspend()
    }
    fn apply_nics(&self, plan: &FixPlan) -> Result<ApplyReport> {
        butterpollo_win_io::wake::apply_nics(plan)
    }
    fn run_fix(&self, plan: &FixPlan) -> Result<Applied> {
        use butterpollo_win_io::wake::{SETTINGS_TIMEOUT, output_within};
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let program = std::env::current_exe()?;
        let mut command = std::process::Command::new(&program);
        command
            .arg("--apply-wake-settings")
            .env(FIX_VARIABLE, serde_json::to_string(plan)?)
            .creation_flags(CREATE_NO_WINDOW);
        // The child gives up on its apply at its own deadline, which never
        // falls in an adapter restart; SETTINGS_TIMEOUT is later, so a kill
        // here never lands in one either.
        let (status, stdout, stderr) = output_within(&mut command, b"", SETTINGS_TIMEOUT)
            .context("butterpollo.exe --apply-wake-settings")?;
        serde_json::from_str(&stdout).with_context(|| {
            format!(
                "--apply-wake-settings ended with {status} without a report: {}",
                stderr.trim()
            )
        })
    }
    fn hold_awake(&self, reason: &str) -> Result<Box<dyn Send>> {
        Ok(Box::new(
            butterpollo_win_io::wake::PowerRequest::system_required(reason)?,
        ))
    }
}

/// `butterpollo.exe --idle-seconds` in the signed-in user's session, where
/// `GetLastInputInfo` sees their input; its exit code.
fn idle_helper() -> Result<u32> {
    use butterpollo_win_io::process::{Process, Target};
    let program = std::env::current_exe()?;
    let helper = Process::spawn(
        &program,
        &["--idle-seconds".into()],
        program.parent(),
        Target::User { elevated: false },
        &Default::default(),
        true,
    )
    .context("starting the idle helper in the signed-in user's session")?;
    helper.wait(IDLE_HELPER_WAIT).inspect_err(|_| {
        let _ = helper.terminate();
    })
}

/// The idle helper's exit code as local input.
fn helper_input(code: u32) -> Input {
    const SECONDS: u32 = 0x000F_FFFF;
    const WIN32: u32 = 0xFFFF;
    if code & !SECONDS == IDLE_READ {
        return Input::Idle(Duration::from_secs((code & SECONDS).into()));
    }
    if code & !WIN32 == IDLE_FAILED {
        let error = std::io::Error::from_raw_os_error((code & WIN32) as i32);
        return Input::Failed(format!("the idle helper failed: {error}"));
    }
    Input::Failed(format!("the idle helper exited with code {code:#x}"))
}

/// `--idle-seconds`: the exit code [`helper_input`] reads.
pub fn idle_seconds_exit_code() -> i32 {
    butterpollo_win_io::wake::idle_exit_code() as i32
}

/// The timers and the last answer of sleep after idle.
struct Idle {
    machine: IdleSleep,
    last: Option<Status>,
    /// When the host last put the PC to sleep, and the error if it failed.
    last_sleep: Option<(SystemTime, Option<String>)>,
}

/// The wake grace and its power request while it holds the PC awake.
#[derive(Default)]
struct Grace {
    machine: WakeGrace,
    held: Option<Box<dyn Send>>,
    /// The sleep clock's error was logged.
    clock_failed: bool,
}

/// The host's wake state, in `Host::wake`.
pub struct Wake {
    power: RwLock<Arc<dyn PowerSystem>>,
    idle: Mutex<Idle>,
    /// The last authenticated console request.
    pub console: Mutex<Option<Instant>>,
    /// The last paired client's HTTPS request.
    client: Mutex<Option<Instant>>,
    grace: Mutex<Grace>,
    nics: Mutex<Option<(Instant, Result<NicReport, String>)>>,
    fix: Mutex<Fix>,
    /// The hash of the last `/wake` body and its revision.
    revision: Mutex<(u64, u64)>,
}

impl Default for Wake {
    fn default() -> Self {
        Self {
            power: RwLock::new(Arc::new(Windows)),
            idle: Mutex::new(Idle {
                machine: IdleSleep::new(Instant::now()),
                last: None,
                last_sleep: None,
            }),
            console: Mutex::new(None),
            client: Mutex::new(None),
            grace: Mutex::default(),
            nics: Mutex::new(None),
            fix: Mutex::default(),
            revision: Mutex::new((0, 0)),
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Wake {
    pub(crate) fn power(&self) -> Arc<dyn PowerSystem> {
        self.power
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_power(&self, power: Arc<dyn PowerSystem>) {
        *self.power.write().unwrap() = power;
    }
    /// The NIC settings, read at most [`NIC_CACHE`] ago unless `refresh`.
    /// Concurrent readers wait for one reading.
    fn nics(&self, power: &dyn PowerSystem, refresh: bool) -> Result<NicReport, String> {
        let mut cache = locked(&self.nics);
        if !refresh
            && let Some((at, report)) = cache.as_ref()
            && at.elapsed() < NIC_CACHE
        {
            return report.clone();
        }
        let report = power.read_nics().map_err(|error| {
            tracing::warn!("Wake-on-LAN: the NIC wake settings could not be read: {error:#}");
            format!("{error:#}")
        });
        *cache = Some((Instant::now(), report.clone()));
        report
    }
    /// The revision of `wake`, raised whenever anything but the per-client
    /// `primary` and `route` changed.
    fn revision(&self, wake: &api::Wake) -> u64 {
        let mut shared = wake.clone();
        shared.primary = None;
        for target in &mut shared.targets {
            target.route = false;
        }
        let mut hasher = DefaultHasher::new();
        serde_json::to_string(&shared)
            .unwrap_or_default()
            .hash(&mut hasher);
        let hash = hasher.finish();
        let mut revision = locked(&self.revision);
        if revision.1 == 0 || revision.0 != hash {
            *revision = (hash, revision.1 + 1);
        }
        revision.1
    }
}

/// An authenticated console request: sleep after idle waits a full interval
/// after the last one.
pub fn note_console_activity(h: &Host) {
    *locked(&h.wake.console) = Some(Instant::now());
}

/// A paired client's HTTPS request: after a resume, the PC stays awake while
/// clients connect (A.4).
pub fn note_client_request(h: &Host) {
    let now = Instant::now();
    *locked(&h.wake.client) = Some(now);
    // The caller may hold the sessions: a stream that started ends the grace
    // at the next tick.
    grace_at(h, &*h.wake.power(), now, false);
}

/// Take or drop the wake grace's power request.
fn grace_at(h: &Host, power: &dyn PowerSystem, now: Instant, streaming: bool) {
    let slept = power.slept();
    let client = locked(&h.wake.client).map(|at| now.saturating_duration_since(at));
    let mut grace = locked(&h.wake.grace);
    match &slept {
        Err(error) if !grace.clock_failed => {
            tracing::warn!("wake grace: the sleep clock is unreadable: {error:#}")
        }
        _ => {}
    }
    grace.clock_failed = slept.is_err();
    let hold = grace.machine.tick(now, slept.ok(), client, streaming);
    if hold && grace.held.is_none() {
        match power.hold_awake(GRACE_REASON) {
            Ok(request) => {
                tracing::info!(
                    "wake grace: a device is connecting after a wake; keeping the PC awake"
                );
                grace.held = Some(request);
            }
            Err(error) => tracing::warn!("wake grace: the power request failed: {error:#}"),
        }
    } else if !hold && grace.held.take().is_some() {
        tracing::info!("wake grace: over; Windows' own sleep timers apply again");
    }
}

/// Sleep after idle's state now: the last tick's, unless the settings changed
/// since.
pub fn status(h: &Host) -> Status {
    let settings = Settings::read(&h.config.read().unwrap_or_else(PoisonError::into_inner));
    let state = |state, minutes, reason: Option<&str>| Status {
        state,
        minutes,
        reason: reason.map(Into::into),
        remaining: None,
        sleep: false,
    };
    match settings {
        Err(error) => state(api::IdleSleepState::Blocked, 0, Some(&error)),
        Ok(settings) if settings.minutes == 0 => state(api::IdleSleepState::Off, 0, None),
        Ok(settings) => locked(&h.wake.idle)
            .last
            .clone()
            .filter(|last| last.minutes == settings.minutes)
            .unwrap_or_else(|| {
                state(
                    api::IdleSleepState::Blocked,
                    settings.minutes,
                    Some("not checked yet"),
                )
            }),
    }
}

/// Whether a stream holds the PC, from the host's own maps: what Sleep PC
/// counts as busy (`power_action::idle`) too, a teardown, a WebRTC transport
/// and a controller session included. `suspended` also counts a launched app
/// and a display kept for a paused stream.
fn holding(h: &Host) -> (bool, bool) {
    let sessions = locked(&h.sessions);
    let streaming = !sessions.active.is_empty()
        || !sessions.pending.is_empty()
        || !sessions.teardown.is_empty();
    drop(sessions);
    let streaming = streaming || !h.webrtc_links.is_empty() || h.controller_sessions.in_use();
    let suspended = locked(&h.current_app).is_some() || !locked(&h.monitors).is_empty();
    (streaming, suspended)
}

fn updating(h: &Host) -> bool {
    matches!(
        locked(&h.updates)["phase"].as_str(),
        Some("waiting" | "downloading" | "ready" | "installing")
    )
}

/// One sleep-after-idle check, every [`idle::TICK`] on the blocking pool.
pub fn tick(h: &Host) {
    tick_at(h, Instant::now());
}

/// What sleep after idle sees `now`, with the session in `state`. The
/// recheck passes the `first` snapshot of the same tick: it holds the launch
/// gate, so it keeps that one's input and power capabilities instead of
/// starting the idle helper (up to [`IDLE_HELPER_WAIT`]) again under it.
fn snapshot(
    h: &Host,
    power: &dyn PowerSystem,
    now: Instant,
    state: Session,
    first: Option<&Snapshot>,
) -> Snapshot {
    let settings = Settings::read(&h.config.read().unwrap_or_else(PoisonError::into_inner));
    let on = settings.as_ref().is_ok_and(|settings| settings.minutes > 0);
    let (mut streaming, mut suspended) = holding(h);
    streaming |= !matches!(state, Session::Idle | Session::Suspended);
    suspended |= state == Session::Suspended;
    // Its own statement: a guard in the literal below would be held while the
    // helpers run, and every console request would wait for them.
    let console = locked(&h.wake.console).map(|at| now.saturating_duration_since(at));
    // Off asks Windows nothing and starts no helper.
    Snapshot {
        now,
        settings,
        system: match first {
            Some(first) => first.system,
            None => on
                .then(|| {
                    power.system_facts().inspect_err(|error| {
                        tracing::warn!("sleep after idle: {error:#}");
                    })
                })
                .and_then(Result::ok),
        },
        streaming,
        suspended,
        updating: updating(h),
        console,
        execution: if on {
            power
                .execution_state()
                .map_err(|error| format!("{error:#}"))
        } else {
            Ok(0)
        },
        input: if on && !streaming {
            first.map_or_else(|| power.input(), |first| first.input.clone())
        } else {
            Input::Failed("not read".into())
        },
        slept: if on {
            power.slept().map_err(|error| format!("{error:#}"))
        } else {
            Ok(Duration::ZERO)
        },
    }
}

fn tick_at(h: &Host, now: Instant) {
    let power = h.wake.power();
    let snapshot = snapshot(h, &*power, now, h.session_snapshot().state, None);
    grace_at(h, &*power, now, snapshot.streaming);
    let mut idle = locked(&h.wake.idle);
    let status = idle.machine.tick(&snapshot);
    let previous = idle.last.replace(status.clone());
    drop(idle);
    if previous
        .as_ref()
        .is_none_or(|previous| previous.state != status.state || changed(previous, &status))
    {
        log_status(&status);
    }
    if !status.sleep {
        return;
    }
    // A launch or teardown holds this; no stream may start while the PC goes
    // down. The session lock is not waited for under it: an action holds
    // that one while it waits for this.
    let Ok(_transition) = h.launch_transition.try_lock() else {
        tracing::info!("sleep after idle: a launch or teardown is running; checking again later");
        return;
    };
    let Some(state) = h.try_session_state() else {
        tracing::info!("sleep after idle: a session action is running; checking again later");
        return;
    };
    // Everything again, now that no launch can start: a stream, console
    // activity or an app's power request may have come since.
    let snapshot = self::snapshot(h, &*power, now, state, Some(&snapshot));
    let mut idle = locked(&h.wake.idle);
    let status = idle.machine.tick(&snapshot);
    idle.last = Some(status.clone());
    if !status.sleep {
        drop(idle);
        tracing::info!(
            reason = status.reason.as_deref().unwrap_or(""),
            "sleep after idle: something changed at the last moment; not sleeping"
        );
        return;
    }
    idle.machine.attempted(now);
    drop(idle);
    tracing::warn!(
        minutes = status.minutes,
        "sleep after idle: idle for {} minutes; putting the PC to sleep",
        status.minutes
    );
    // When it went to sleep, not when it came back.
    let at = SystemTime::now();
    let result = power.sleep();
    if let Err(error) = &result {
        tracing::error!("sleep after idle: the PC did not sleep: {error:#}");
    } else {
        tracing::info!("sleep after idle: the PC resumed");
    }
    locked(&h.wake.idle).last_sleep = Some((at, result.err().map(|error| format!("{error:#}"))));
}

/// A new blocking or unsupported reason; an armed countdown's reason changes
/// on its own.
fn changed(previous: &Status, status: &Status) -> bool {
    status.state != api::IdleSleepState::Armed && previous.reason != status.reason
}

fn log_status(status: &Status) {
    let reason = status.reason.as_deref().unwrap_or("");
    match status.state {
        api::IdleSleepState::Unsupported => {
            tracing::warn!("sleep after idle is unsupported: {reason}")
        }
        api::IdleSleepState::Blocked => tracing::info!("sleep after idle is blocked: {reason}"),
        api::IdleSleepState::Armed => tracing::info!(
            minutes = status.minutes,
            "sleep after idle is armed: the PC sleeps after {} idle minutes",
            status.minutes
        ),
        _ => tracing::info!("sleep after idle is off"),
    }
}

/// One reading of the adapters, the NIC settings and the power facts.
struct Reading {
    adapters: Vec<Adapter>,
    nics: Result<NicReport, String>,
    facts: Result<SystemFacts, String>,
}

fn read(h: &Host, refresh: bool) -> Result<Reading> {
    let power = h.wake.power();
    let adapters = power.adapters().context("reading the network adapters")?;
    let nics = h.wake.nics(&*power, refresh);
    let facts = power.system_facts().map_err(|error| format!("{error:#}"));
    Ok(Reading {
        adapters,
        nics,
        facts,
    })
}

fn wake_of(h: &Host, reading: &Reading, local: Option<IpAddr>) -> api::Wake {
    let facts = reading.facts.as_ref().ok();
    // Loopback gets zeros in /serverinfo, so no primary here either.
    let local = local
        .map(|local| local.to_canonical())
        .filter(|local| !local.is_loopback());
    let (primary, targets) = readiness::targets(
        &reading.adapters,
        reading.nics.as_ref().map_err(String::as_str),
        facts.is_some_and(|facts| facts.modern_standby),
        local,
    );
    let mut wake = api::Wake {
        revision: 0,
        primary,
        targets,
        system: readiness::system(facts),
        idle_sleep: status(h).api(),
    };
    wake.revision = h.wake.revision(&wake);
    wake
}

/// `GET /wake` for a request that reached the PC at `local`.
pub fn readiness(h: &Host, refresh: bool, local: Option<IpAddr>) -> Result<api::Wake> {
    Ok(wake_of(h, &read(h, refresh)?, local))
}

#[cfg(test)]
mod tests;
