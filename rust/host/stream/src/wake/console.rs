//! Console status and diagnostic snapshots for Wake-on-LAN and idle sleep.

use super::{PowerSystem, Windows, locked, read, status, wake_of};
use crate::state::Host;
use anyhow::Result;
use butterpollo_session::wake::{self as readiness, FixPlan, SystemFacts, idle};
use serde_json::{Value, json};
use std::{net::IpAddr, time::SystemTime};

fn unix(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Where the console judges readiness from. /serverinfo and `/wake` give
/// loopback zeros, but a console opened on this PC asks about the adapter a
/// device on the network would reach: the one under the default route.
fn console_local(h: &Host, local: Option<IpAddr>) -> Result<Option<IpAddr>, Missing> {
    let Some(local) = local.map(|local| local.to_canonical()) else {
        return Ok(None);
    };
    if !local.is_loopback() {
        return Ok(Some(local));
    }
    match h.wake.power().default_route() {
        Ok(Some(route)) => Ok(Some(route)),
        Ok(None) => Err(Missing::NoRoute),
        Err(error) => {
            tracing::warn!("Wake-on-LAN: the default route could not be read: {error:#}");
            Err(Missing::RouteError(format!("{error:#}")))
        }
    }
}

/// Why the console has no primary adapter.
#[derive(Debug)]
enum Missing {
    /// Opened on this PC, which has no default route.
    NoRoute,
    /// Opened on this PC, whose default route could not be read.
    RouteError(String),
    /// The address the request reached, or the default route's, leads to no
    /// connected physical adapter.
    Unmapped,
}

impl Missing {
    fn json(&self) -> Value {
        match self {
            Self::NoRoute => json!({"reason": "no_route", "error": null}),
            Self::RouteError(error) => json!({"reason": "route_error", "error": error}),
            Self::Unmapped => json!({"reason": "unmapped", "error": null}),
        }
    }
}

/// `GET /api/power`: readiness as `/wake` has it, the changes a fix would
/// make, sleep after idle with its countdown, and the last sleep and fix.
pub fn console_status(h: &Host, refresh: bool, local: Option<IpAddr>) -> Result<Value> {
    let reading = read(h, refresh)?;
    let local = console_local(h, local);
    let wake = wake_of(h, &reading, local.as_ref().ok().copied().flatten());
    let missing = wake
        .primary
        .is_none()
        .then(|| local.err().unwrap_or(Missing::Unmapped));
    let plan = match (&reading.nics, &reading.facts) {
        (Ok(nics), Ok(facts)) => Ok(FixPlan::new(&reading.adapters, nics, facts, false)),
        (Err(error), _) | (_, Err(error)) => Err(error.clone()),
    };
    let fast_startup = reading
        .facts
        .as_ref()
        .is_ok_and(SystemFacts::fast_startup_active);
    let idle = status(h);
    let (last_sleep, last_fix, running) = {
        let last_sleep = locked(&h.wake.idle).last_sleep.clone();
        let fix = locked(&h.wake.fix);
        (last_sleep, fix.last.clone(), fix.running)
    };
    Ok(json!({
        "status": true,
        "wake": wake,
        // Why `wake.primary` is null; null when it isn't.
        "primary_missing": missing.as_ref().map(Missing::json),
        "idle_sleep": {
            "state": idle.state,
            "minutes": idle.minutes,
            "reason": idle.reason,
            "remaining_seconds": idle.remaining.map(|left| left.as_secs()),
        },
        "last_sleep": last_sleep.map(|(at, error)| json!({"at": unix(at), "error": error})),
        "fix": {
            "running": running,
            "changes": plan.as_ref().ok().map(FixPlan::changes),
            "skipped": plan.as_ref().ok().map(|plan| &plan.skipped),
            "error": plan.as_ref().err(),
            "fast_startup_off_offered": fast_startup,
            "last": last_fix.map(|(at, result)| match result {
                Ok(applied) => json!({
                    "at": unix(at),
                    "changes": applied.changes,
                    "restarted": applied.restarted,
                    "after_reboot": applied.after_reboot,
                    "errors": applied.errors,
                }),
                Err(error) => json!({"at": unix(at), "errors": [error]}),
            }),
        },
    }))
}

/// The `wake` field of `--diagnostics`.
pub fn diagnostics() -> Value {
    let text = |error: anyhow::Error| format!("{error:#}");
    let power = Windows;
    let adapters = power.adapters().map_err(text);
    let nics = power.read_nics().map_err(text);
    let facts = power.system_facts().map_err(text);
    let execution = power.execution_state().map_err(text);
    let targets = adapters.as_ref().ok().map(|adapters| {
        readiness::targets(
            adapters,
            nics.as_ref().map_err(String::as_str),
            facts.as_ref().is_ok_and(|facts| facts.modern_standby),
            None,
        )
        .1
    });
    let changes = match (&adapters, &nics, &facts) {
        (Ok(adapters), Ok(nics), Ok(facts)) => {
            Some(FixPlan::new(adapters, nics, facts, false).changes())
        }
        _ => None,
    };
    json!({
        "adapters": adapters,
        "targets": targets,
        "nic_script_error": nics.as_ref().err(),
        "system": readiness::system(facts.as_ref().ok()),
        "power_facts": facts,
        "kept_awake": execution.map(idle::kept_awake),
        "fix": changes,
    })
}
