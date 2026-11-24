//! Wake-on-LAN adapter fix planning, execution, and console job tracking.

use super::{PowerSystem, Windows, holding, locked, read};
use crate::state::Host;
use anyhow::{Context, Result, bail};
use butterpollo_session::wake::FixPlan;
use serde::{Deserialize, Serialize};
use std::{
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, SystemTime},
};

/// The console's POST answers before the fix starts, as the adapter restart
/// drops the connection it came on.
pub const FIX_DELAY: Duration = Duration::from_secs(1);
/// The console's [`FixPlan`] for `--apply-wake-settings`, as JSON: the child
/// applies exactly that plan instead of planning again.
pub(super) const FIX_VARIABLE: &str = "BUTTERPOLLO_WAKE_FIX";

/// What `--apply-wake-settings` changed; it prints this as JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Applied {
    /// The plan, in the words the console showed.
    pub changes: Vec<String>,
    /// The adapters restarted for a changed advanced property.
    pub restarted: Vec<String>,
    /// The adapters whose changed property takes effect when the PC restarts.
    pub after_reboot: Vec<String>,
    pub errors: Vec<String>,
}

/// `--apply-wake-settings`: apply the host's plan in [`FIX_VARIABLE`], or
/// without one (setup) the plan for this PC now, print what changed as JSON
/// and exit 1 if anything failed.
pub fn apply_settings_exit_code(fast_startup_off: bool) -> i32 {
    let applied = match std::env::var(FIX_VARIABLE) {
        Ok(plan) => serde_json::from_str(&plan)
            .context("reading the host's Wake-on-LAN plan")
            .and_then(|plan| apply_plan(&Windows, &plan)),
        Err(std::env::VarError::NotPresent) => apply_settings(&Windows, fast_startup_off),
        Err(error) => Err(error).context(FIX_VARIABLE),
    };
    let applied = applied.unwrap_or_else(|error| Applied {
        errors: vec![format!("{error:#}")],
        ..Default::default()
    });
    println!(
        "{}",
        serde_json::to_string(&applied).expect("Applied serializes")
    );
    i32::from(!applied.errors.is_empty())
}

/// Read this PC's adapters and settings, plan the changes and apply them.
pub fn apply_settings(power: &dyn PowerSystem, fast_startup_off: bool) -> Result<Applied> {
    let adapters = power.adapters().context("reading the network adapters")?;
    let report = power.read_nics().context("reading the NIC wake settings")?;
    let facts = power
        .system_facts()
        .context("reading the power capabilities")?;
    apply_plan(
        power,
        &FixPlan::new(&adapters, &report, &facts, fast_startup_off),
    )
}

/// Apply `plan` as it is. An adapter it skipped is an error, and a plan that
/// would set more than wake is refused whole.
pub fn apply_plan(power: &dyn PowerSystem, plan: &FixPlan) -> Result<Applied> {
    if let Some(refusal) = plan.refusal() {
        bail!("the Wake-on-LAN plan was refused: {refusal}");
    }
    let mut applied = Applied {
        changes: plan.changes(),
        errors: plan.skipped.clone(),
        ..Default::default()
    };
    if plan.is_empty() {
        return Ok(applied);
    }
    let report = power.apply_nics(plan)?;
    applied.restarted = report.restarted;
    applied.after_reboot = report.after_reboot;
    applied.errors.extend(report.errors);
    Ok(applied)
}

/// The console's fix: one at a time.
#[derive(Default)]
pub(super) struct Fix {
    pub(super) running: bool,
    pub(super) last: Option<(SystemTime, Result<Applied, String>)>,
}

/// Why the console's fix was refused (409).
#[derive(Debug, PartialEq, Eq)]
pub enum FixRefusal {
    Streaming,
    Running,
}

impl std::fmt::Display for FixRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Streaming => {
                "a stream is active or starting; the network drops while adapters restart, so end it first"
            }
            Self::Running => "the Wake-on-LAN fix is already running",
        })
    }
}

/// Why the console's fix would be refused now, before reading the adapters;
/// [`request_fix`] checks again.
pub fn fix_refusal(h: &Host) -> Option<FixRefusal> {
    if locked(&h.wake.fix).running {
        return Some(FixRefusal::Running);
    }
    refusal_now(h)
}

/// The changes the console's fix would make now, from a fresh reading.
pub fn fix_plan(h: &Host, fast_startup_off: bool) -> Result<FixPlan> {
    let reading = read(h, true)?;
    let nics = reading
        .nics
        .map_err(anyhow::Error::msg)
        .context("reading the NIC wake settings")?;
    let facts = reading
        .facts
        .map_err(anyhow::Error::msg)
        .context("reading the power capabilities")?;
    Ok(FixPlan::new(
        &reading.adapters,
        &nics,
        &facts,
        fast_startup_off,
    ))
}

/// Start the console's fix: after [`FIX_DELAY`], with no stream starting
/// meanwhile, `--apply-wake-settings` applies exactly `plan`, unless the
/// adapters changed since. The result is in [`console_status`].
pub fn request_fix(h: &Arc<Host>, plan: &FixPlan) -> Result<(), FixRefusal> {
    {
        let mut fix = locked(&h.wake.fix);
        if fix.running {
            return Err(FixRefusal::Running);
        }
        if holding(h).0 {
            return Err(FixRefusal::Streaming);
        }
        fix.running = true;
    }
    tracing::warn!(
        changes = ?plan.changes(),
        "Wake-on-LAN: applying the fix the console asked for"
    );
    let started = std::thread::Builder::new().name("wake-fix".into()).spawn({
        let mut run = FixRun {
            h: h.clone(),
            result: None,
        };
        let plan = plan.clone();
        move || {
            std::thread::sleep(FIX_DELAY);
            let result = run_fix(&run.h, &plan);
            run.done(result);
        }
    });
    if let Err(error) = started {
        // The closure went with the thread, and its FixRun finished the fix.
        tracing::error!("Wake-on-LAN: the fix thread did not start: {error}");
    }
    Ok(())
}

/// The fix thread's result, finished when it is dropped: also when the
/// thread panics, so `running` never stays set.
struct FixRun {
    h: Arc<Host>,
    result: Option<Result<Applied, String>>,
}

impl FixRun {
    /// Read by `drop`, which reports it.
    fn done(&mut self, result: Result<Applied, String>) {
        self.result = Some(result);
    }
}

impl Drop for FixRun {
    fn drop(&mut self) {
        let result = self
            .result
            .take()
            .unwrap_or_else(|| Err("the Wake-on-LAN fix stopped unexpectedly".into()));
        finish_fix(&self.h, result);
    }
}

fn run_fix(h: &Host, offered: &FixPlan) -> Result<Applied, String> {
    // The reading may take a minute; launches go on meanwhile.
    let now = fix_plan(h, offered.fast_startup_off).map_err(|error| format!("{error:#}"))?;
    if now != *offered {
        return Err(
            "not applied: the adapters' settings changed since the fix was offered; check again"
                .into(),
        );
    }
    // No launch starts while adapters restart; the session lock is not taken
    // under this (see `tick_at`).
    let _transition = locked(&h.launch_transition);
    if let Some(refusal) = refusal_now(h) {
        return Err(format!("not applied: {refusal}"));
    }
    let power = h.wake.power();
    // A panic here would poison the lock every launch takes.
    std::panic::catch_unwind(AssertUnwindSafe(|| power.run_fix(offered)))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the Wake-on-LAN fix panicked")))
        .map_err(|error| format!("{error:#}"))
}

/// A stream that started between the POST and the fix.
fn refusal_now(h: &Host) -> Option<FixRefusal> {
    holding(h).0.then_some(FixRefusal::Streaming)
}

fn finish_fix(h: &Host, result: Result<Applied, String>) {
    match &result {
        Ok(applied) if applied.errors.is_empty() => tracing::info!(
            changes = ?applied.changes,
            restarted = ?applied.restarted,
            after_reboot = ?applied.after_reboot,
            "Wake-on-LAN: the fix is applied"
        ),
        Ok(applied) => tracing::error!(
            changes = ?applied.changes,
            errors = ?applied.errors,
            "Wake-on-LAN: the fix failed in part"
        ),
        Err(error) => tracing::error!("Wake-on-LAN: the fix failed: {error}"),
    }
    // What the adapters report now is new.
    *locked(&h.wake.nics) = None;
    let mut fix = locked(&h.wake.fix);
    fix.running = false;
    fix.last = Some((SystemTime::now(), result));
}
