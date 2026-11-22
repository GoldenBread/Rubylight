//! App library JSON loading and normalization helpers for [`super`].
use anyhow::{Context, Result};
use butterpollo_session::state::App;
use serde_json::{Value, json};
use std::path::PathBuf;

/// Give each app a uuid and bring its settings to what the host reads now
/// (`migrate_app`); true when anything changed.
pub(super) fn normalize_library(document: &mut Value) -> bool {
    let mut assigned = false;
    if let Some(list) = document.get_mut("apps").and_then(Value::as_array_mut) {
        for app in list.iter_mut().filter_map(Value::as_object_mut) {
            if app
                .get("uuid")
                .and_then(Value::as_str)
                .is_none_or(|uuid| uuid.trim().is_empty())
            {
                app.insert("uuid".into(), json!(uuid::Uuid::new_v4().to_string()));
                assigned = true;
            }
            // A ViGEmBus controller type, say, becomes its VHF pad, and a
            // setting saved under an earlier name takes its current one.
            assigned |= butterpollo_config::config::migrate_app(app);
        }
    }
    assigned
}

/// The app library. One that cannot be read as a library starts the host
/// with no apps, as in Vibepollo, rather than keeping the host and its web
/// console down; the file is copied beside it so a later save cannot lose it.
pub(super) fn load_library(path: &std::path::Path, default: Value) -> Result<Value> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(default),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let Some(json) = butterpollo_session::state::json_bytes(&bytes) else {
        return Ok(default);
    };
    let parsed = serde_json::from_slice::<Value>(json)
        .map_err(anyhow::Error::from)
        .and_then(|document| {
            serde_json::from_value::<Vec<App>>(document.get("apps").cloned().unwrap_or(json!([])))?;
            Ok(document)
        });
    match parsed {
        Ok(document) => Ok(document),
        Err(error) => {
            let mut kept = path.as_os_str().to_owned();
            kept.push(".invalid");
            let kept = PathBuf::from(kept);
            butterpollo_session::state::atomic_write(&kept, &bytes)?;
            tracing::error!(
                error = %format!("{error:#}"),
                file = %path.display(),
                kept = %kept.display(),
                "the app library is not valid; starting with no apps"
            );
            Ok(json!({"env":{},"apps":[]}))
        }
    }
}
