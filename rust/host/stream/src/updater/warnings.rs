//! Persisted update check timestamps, automatic install attempts, and failure warnings.

use crate::state::Shared;
use butterpollo_session::version::newer;
use serde_json::{Value, json};
use std::sync::atomic::AtomicU64;

/// The state file's entry for the last successful release check.
pub(super) const LAST_CHECK: &str = "last_update_check";

/// The last successful release check before this run, as Unix seconds; 0
/// when there was none.
pub(super) fn remembered_check(h: &Shared) -> u64 {
    h.aliases.lock().unwrap()["root"][LAST_CHECK]
        .as_u64()
        .unwrap_or(0)
}

/// Keep the moment of the release check that just succeeded across restarts,
/// for the daily check at a time of day ([`schedule`]).
pub fn remember_check(h: &Shared) {
    let checked_at = h.updates.lock().unwrap()["checked_at"]
        .as_u64()
        .unwrap_or(0);
    let mut aliases = h.aliases.lock().unwrap();
    let mut next = aliases.clone();
    if !next["root"].is_object() {
        next["root"] = json!({});
    }
    next["root"][LAST_CHECK] = json!(checked_at);
    match butterpollo_session::state::write_json(&h.aliases_path, &next) {
        Ok(()) => *aliases = next,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "the release check time could not be saved");
        }
    }
}

/// The state file's entry naming the release an automatic install started, until its outcome
/// is known: a host setup restarted after a rollback still reports the failure.
pub(super) const AUTOMATIC_ATTEMPT: &str = "automatic_update_attempt";
/// The host warning for an automatic update that could not be installed.
pub const UPDATE_FAILED: &str = "update_failed";

/// The state file's entry naming the release [`UPDATE_FAILED`] reports, until that release or
/// a later one is installed.
pub(super) const FAILED_VERSION: &str = "failed_update_version";

/// Names the release an automatic install starts, or forgets it (`None`).
pub(super) fn remember_attempt(h: &Shared, version: Option<&str>) {
    remember(h, AUTOMATIC_ATTEMPT, version);
}
/// Sets the state file's entry `key` to `version`, or removes it (`None`).
fn remember(h: &Shared, key: &str, version: Option<&str>) {
    let mut aliases = h.aliases.lock().unwrap();
    let mut next = aliases.clone();
    if !next["root"].is_object() {
        next["root"] = json!({});
    }
    match version {
        Some(version) => next["root"][key] = json!(version),
        None => {
            if let Some(root) = next["root"].as_object_mut() {
                root.remove(key);
            }
        }
    }
    match butterpollo_session::state::write_json(&h.aliases_path, &next) {
        Ok(()) => *aliases = next,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "the automatic update attempt could not be saved");
        }
    }
}

/// When `poll` next has `report_failed_install` read setup's record.
pub(super) static NEXT_REPORT: AtomicU64 = AtomicU64::new(0);
/// Raises [`UPDATE_FAILED`] once for the automatic install `remember_attempt` named, when it
/// failed: with `error` when this host saw setup fail, else from setup's record, which the
/// host setup started again after a rollback reads. Nothing while setup has not written its
/// outcome; a release installed or replaced by another install is forgotten quietly.
pub(super) fn report_failed_install(h: &Shared, error: Option<String>) {
    let Some(version) = h.aliases.lock().unwrap()["root"][AUTOMATIC_ATTEMPT]
        .as_str()
        .map(str::to_owned)
    else {
        return;
    };
    let reason = match error {
        Some(error) => Some(error),
        None => {
            let record = butterpollo_session::state::load_json(
                &h.directory.join("update-result.json"),
                Value::Null,
            )
            .unwrap_or(Value::Null);
            let same = record["version"]
                .as_str()
                .is_some_and(|v| v.trim_start_matches('v') == version.trim_start_matches('v'));
            match record["phase"].as_str() {
                Some("installing") if same => return,
                Some("failed" | "rolled_back" | "recovery_failed") if same => Some(
                    record["error"]
                        .as_str()
                        .unwrap_or("setup did not finish")
                        .to_owned(),
                ),
                _ => None,
            }
        }
    };
    remember_attempt(h, None);
    if let Some(reason) = reason {
        raise_failed_install(h, &version, &reason);
    }
}
/// Raises [`UPDATE_FAILED`] for `version`, saved until it is dismissed or that release (or a
/// later one) is installed ([`withdraw_installed_warning`]).
pub(super) fn raise_failed_install(h: &Shared, version: &str, reason: &str) {
    remember(h, FAILED_VERSION, Some(version));
    crate::host_warnings::notice(
        &h.directory,
        &h.warnings,
        UPDATE_FAILED,
        &failed_install_warning(version, reason),
    );
}
/// Takes [`UPDATE_FAILED`] back once the release it names is installed after all: this host
/// runs it or a later one, or setup's record says a release at least as new was installed.
pub(super) fn withdraw_installed_warning(h: &Shared) {
    let Some(failed) = h.aliases.lock().unwrap()["root"][FAILED_VERSION]
        .as_str()
        .map(str::to_owned)
    else {
        return;
    };
    let record =
        butterpollo_session::state::load_json(&h.directory.join("update-result.json"), Value::Null)
            .unwrap_or(Value::Null);
    let installed = record["phase"] == "installed"
        && record["version"]
            .as_str()
            .is_some_and(|version| !newer(&failed, version));
    if installed || !newer(&failed, env!("CARGO_PKG_VERSION")) {
        crate::host_warnings::withdraw(&h.directory, &h.warnings, UPDATE_FAILED);
        remember(h, FAILED_VERSION, None);
    }
}
/// What the console says of an automatic update that failed, and what to do.
fn failed_install_warning(version: &str, reason: &str) -> String {
    format!(
        "The automatic update to Rubylight {} could not be installed: {}. This PC keeps the version it has; install the update from Updates in the console's settings when the PC is free.",
        version.trim_start_matches('v'),
        reason.trim_end().trim_end_matches('.')
    )
}

/// Why setup exited without replacing this host: what it recorded when it
/// refused before changing anything, else only how it exited.
pub(super) fn setup_failure(
    profile: &std::path::Path,
    exit: std::io::Result<std::process::ExitStatus>,
) -> anyhow::Error {
    let record =
        butterpollo_session::state::load_json(&profile.join("update-result.json"), Value::Null)
            .unwrap_or(Value::Null);
    if record["phase"] == "failed"
        && let Some(error) = record["error"].as_str()
    {
        return anyhow::anyhow!("{error}");
    }
    let exit = match exit {
        Ok(status) => status
            .code()
            .map_or_else(|| status.to_string(), |code| format!("exit code {code}")),
        Err(error) => error.to_string(),
    };
    anyhow::anyhow!("Installer exited before replacing this host ({exit}). See the setup log.")
}
