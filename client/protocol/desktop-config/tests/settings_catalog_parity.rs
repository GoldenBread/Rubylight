//! Snapshot parity tests between generated desktop `Settings::default()` and `settings.toml`.

use std::collections::BTreeSet;

use nightjar_desktop_config::config::{
    DESKTOP_OVERRIDE_KEYS, DESKTOP_SETTING_KEYS, Settings,
};
use nightjar_policy::settings_catalog_gen as cat;

#[test]
fn desktop_default_serializes_every_catalog_field_except_skipped_and_retired() {
    let value = serde_json::to_value(Settings::default()).expect("Settings::default serializes");
    let serialized: BTreeSet<&str> = value
        .as_object()
        .expect("Settings serializes to JSON object")
        .keys()
        .map(String::as_str)
        .collect();

    // `packet_size` is #[serde(skip)]; the three `retired_*` fields are #[serde(skip_serializing)].
    let skipped = BTreeSet::from([
        "packet_size",
        "retired_busy_bitrate",
        "retired_lost_bitrate",
        "retired_keep_bitrate",
    ]);
    let expected: BTreeSet<&str> = DESKTOP_SETTING_KEYS
        .iter()
        .copied()
        .filter(|k| !skipped.contains(k))
        .collect();

    assert_eq!(serialized, expected);
    assert_eq!(DESKTOP_SETTING_KEYS.len(), cat::DESKTOP_SETTING_COUNT);
    assert!(!DESKTOP_OVERRIDE_KEYS.is_empty());
    for key in DESKTOP_OVERRIDE_KEYS {
        assert!(expected.contains(key), "override key {key} missing from serialized Settings");
    }
}

#[test]
fn desktop_default_round_trips_losslessly() {
    let defaults = Settings::default();
    let json = serde_json::to_string(&defaults).unwrap();
    let decoded: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, defaults);
}
