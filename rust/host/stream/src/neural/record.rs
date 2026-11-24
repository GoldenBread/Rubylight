//! Durable `neural.json` user-acceptance and pending-removal record I/O.
use super::{ACCEPTED, SETTINGS_FILE};
use crate::state::Shared;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::sync::PoisonError;

/// The record, empty only when there is none yet: one that can't be read
/// is an error, never taken for empty and written over (it holds the
/// acceptance and the pending removals).
fn read_settings(h: &Shared) -> Result<Value> {
    let path = h.directory.join(SETTINGS_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let value: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("{} is damaged", path.display()))?;
    ensure!(
        value.is_object(),
        "{} is damaged: no object",
        path.display()
    );
    Ok(value)
}

/// [`read_settings`] for a reader, empty and logged when it can't be read.
pub fn settings(h: &Shared) -> Value {
    read_settings(h).unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: its record can't be read");
        json!({})
    })
}

/// Change the record and write it; nothing is written when `change` fails.
pub(super) fn update_settings(
    h: &Shared,
    change: impl FnOnce(&mut Value) -> Result<()>,
) -> Result<()> {
    let _one = h
        .neural
        .record
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut value = read_settings(h)?;
    change(&mut value)?;
    butterpollo_session::state::write_json(&h.directory.join(SETTINGS_FILE), &value)
}

/// Whether the user accepted the notice that comes before the first download.
pub fn accepted(h: &Shared) -> bool {
    settings(h)[ACCEPTED].as_u64().is_some()
}

pub fn accept(h: &Shared) -> Result<()> {
    update_settings(h, |value| {
        value[ACCEPTED] = json!(now());
        Ok(())
    })?;
    tracing::info!("Neural Rendering: the user accepted the third-party frame generation notice");
    Ok(())
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
