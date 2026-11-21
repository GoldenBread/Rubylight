//! Host update and lifecycle API handlers.

use crate::state::Shared;
use serde_json::{Value, json};

pub(super) fn handle(h: &Shared, method: &str, path: &str) -> anyhow::Result<Value> {
    Ok(match (method, path) {
        ("GET", "/api/updates") => crate::updater::status(h),
        ("POST", "/api/updates/install") => {
            crate::updater::queue(h, false, false, None)?;
            json!({"status":true})
        }
        ("POST", "/api/updates/install_now") => {
            crate::updater::queue(h, false, true, None)?;
            json!({"status":true})
        }
        ("POST", "/api/updates/cancel") => {
            crate::updater::cancel(h)?;
            json!({"status":true})
        }
        ("POST", "/api/updates/check") => {
            crate::maintenance::trigger_update(h, false);
            json!({"status":true})
        }
        ("POST", "/api/restart") => {
            tracing::info!("host restart requested from the administration console");
            h.request_stop("console_restart", true);
            json!({"status":true})
        }
        ("POST", "/api/quit") => {
            tracing::info!("host shutdown requested from the administration console");
            h.request_stop("console_quit", false);
            json!({"status":true})
        }
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}
