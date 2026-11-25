//! Parity tests between `settings.toml`, `settings_catalog_gen`, and shared `nightjar-policy` constants.

use nightjar_policy::{quality, recovery, settings_catalog_gen as cat};
use std::collections::HashSet;

#[test]
fn generated_catalog_matches_shared_policy_constants() {
    assert_eq!(cat::QUALITY_MIN_SHORT_EDGES, quality::MIN_SHORT_EDGE_CHOICES);
    assert_eq!(cat::QUALITY_MIN_FPS, quality::MIN_FPS_CHOICES);
    assert_eq!(cat::NETWORK_GRACE_CHOICES_S, recovery::NETWORK_GRACE_CHOICES_S);
    assert_eq!(cat::DEFAULT_GUIDE_MENU_HOLD_MS, 1000);
    assert!(cat::GUIDE_HOLD_MS.contains(&cat::DEFAULT_GUIDE_MENU_HOLD_MS));
    assert_eq!(cat::DECODE_QUEUE_FRAMES_DEFAULT, 15);
    assert_eq!(cat::RENDER_SCALE, 50..=200);
    assert_eq!(cat::DEADZONE, 0..=50);
    assert_eq!(cat::MIC_GAIN, 25..=400);
    assert_eq!(cat::MIC_GAIN_STEP, 5);
}

#[test]
fn retired_and_shared_catalog_tables_are_well_formed() {
    let android_retired: HashSet<&str> = cat::ANDROID_RETIRED_KEYS.iter().map(|r| r.key).collect();
    assert_eq!(android_retired.len(), cat::ANDROID_RETIRED_KEYS.len());
    assert!(cat::is_android_retired("checkbox_gl_render"));
    assert!(cat::is_android_retired("checkbox_auto_bitrate"));
    assert!(cat::is_android_retired("list_osc_style"));
    assert!(!cat::is_android_retired("list_resolution"));

    let desktop_retired: HashSet<&str> = cat::DESKTOP_RETIRED_KEYS.iter().map(|r| r.key).collect();
    assert_eq!(desktop_retired.len(), cat::DESKTOP_RETIRED_KEYS.len());
    assert!(cat::is_desktop_retired("adaptive_bitrate"));
    assert!(cat::is_desktop_retired("client_hint_bitrate"));
    assert!(cat::is_desktop_retired("keep_bitrate"));
    assert!(!cat::is_desktop_retired("lower_bitrate"));

    assert_eq!(
        cat::ANDROID_RENAMED_KEYS,
        &[("checkbox_auto_bitrate", "adaptive_bitrate")]
    );
    assert!(cat::SHARED_SETTINGS.len() >= 80);
    for shared in cat::SHARED_SETTINGS {
        assert!(!shared.android_key.is_empty());
        assert!(!shared.desktop_key.is_empty());
        assert!(matches!(shared.tier, "main" | "more"));
        assert!(matches!(shared.scope, "global" | "layer"));
    }
}
