//! Scenario unit tests for host configuration parsing, accessors, validation, and migrations.
use super::retired;
use super::*;

mod accessors;
mod catalog_parity;
mod folded_settings;
mod pacing_bitrate;
mod retired_values;
mod validation_parse;

/// A change saved to sunshine.conf, as the settings API does it.
fn change(config: &mut Config, key: &str, value: &str) -> Result<()> {
    let mut object = serde_json::Map::new();
    object.insert(key.into(), value.into());
    let mut next = config.clone();
    next.update(&object)?;
    next.check_saved(object.keys())?;
    *config = next;
    Ok(())
}
