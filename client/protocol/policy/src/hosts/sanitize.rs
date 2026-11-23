//! Shared host-list JSON sanitization so a single unreadable setting or host entry never costs
//! the remaining paired hosts.

use serde_json::{Map, Value};

/// Sanitizes a parsed `hosts.json` array in place: runs `keep_settings` on each host's
/// `settings` and `app_settings` maps, then retains only entries passing `is_valid_host`.
pub fn keep_readable_hosts<F>(
    hosts: &mut Value,
    mut keep_settings: F,
    is_valid_host: fn(&Value) -> bool,
) where
    F: FnMut(&mut Value),
{
    let Some(hosts) = hosts.as_array_mut() else {
        return;
    };
    for host in hosts.iter_mut() {
        if let Some(settings) = host.get_mut("settings").filter(|s| !s.is_null()) {
            keep_settings(settings);
        }
        match host.get_mut("app_settings") {
            Some(Value::Object(games)) => games.values_mut().for_each(&mut keep_settings),
            Some(other) => *other = Value::Object(Map::new()),
            None => {}
        }
    }
    hosts.retain(is_valid_host);
}
