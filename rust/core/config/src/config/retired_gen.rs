//! Generated tables of retired, renamed, removed, and folded host settings.
//! Generated from `rust/core/config/settings.toml` by `tools/generated_docs.py`; do not edit by hand.

/// Setting values Rubylight no longer offers, and what replaces each.
pub const RETIRED_VALUES: &[(&str, &str, &str)] = &[
    ("gamepad", "x360", "vhf_xbox_one"),
    ("gamepad", "ds4", "vhf_ds4"),
];

/// Settings saved under an earlier name and the name the host reads now.
pub const RENAMED_KEYS: &[(&str, &str)] = &[
    ("rtss_path", "rtss_install_path"),
    ("nvenc_force_split_encode", "nvenc_split_encode"),
    ("rtx_hdr_peak_brightness", "hdr_peak_brightness"),
    (
        "frame_limiter_auto_virtual_framegen",
        "virtual_display_refresh",
    ),
    ("experimental_packed_444", "packed_444"),
];

/// Launchers whose sync options earlier releases kept under per-launcher keys.
pub const LIBRARY_PREFIXES: &[&str] = &[
    "steam",
    "epic",
    "battlenet",
    "microsoft",
    "ea",
    "ubisoft",
    "gog",
    "library_local",
    "playnite",
];

/// Launchers that record when a game was played.
pub const LIBRARY_HISTORY_PREFIXES: &[&str] = &["steam", "battlenet", "playnite"];

/// Shared Library sync options: `(name, default, prefixes)`.
pub const LIBRARY_SYNC_OPTIONS: &[(&str, &str, &[&str])] = &[
    ("auto_sync", "true", LIBRARY_PREFIXES),
    ("autosync_remove_uninstalled", "true", LIBRARY_PREFIXES),
    ("sync_all_installed", "true", LIBRARY_HISTORY_PREFIXES),
    ("recent_games", "10", LIBRARY_HISTORY_PREFIXES),
    ("recent_max_age_days", "30", LIBRARY_HISTORY_PREFIXES),
];

/// Settings of features Rubylight removed, dropped silently on load.
pub const REMOVED_KEYS: &[&str] = &[
    "adaptive_fec",
    "fec_max_percentage",
    "frame_limiter_disable_vsync",
    "rtss_disable_vsync_ullm",
    "dd_wa_dummy_plug_hdr10",
    "min_threads",
    "crf",
    "qp",
    "child_session_resolution",
    "child_session_scale",
    "child_session_port",
    "child_session_host",
    "dd_vdd_static_monitor_count",
    "dd_virtual_display_permanent_count",
    "touch_keyboard_digitizer",
    "host_adaptive_bitrate",
    "phase_lock_hold_at_stream_rate",
    "compute_queue_realtime",
    "amd_input_queue_size",
    "amd_lowlatency_mode",
];

/// Prefixes of removed Quick Sync and software encoders.
pub const REMOVED_PREFIXES: &[&str] = &["qsv_", "sw_"];

/// Settings folded into another setting.
pub const FOLDED_KEYS: &[&str] = &[
    "dd_hdr_request_override",
    "link_rate_adaptation",
    "congestion_control",
    "pacing_max_bitrate_kbps",
    "host_adaptive_pacing",
    "wireless_adaptive_pacing",
    "remote_monitor_disconnect_on_stream_end",
    "remote_monitor_disconnect_on_client_disconnect",
    "dd_config_revert_on_disconnect",
    "nvenc_enable_min_qp",
    "auto_update",
];

/// Minimum QP keys and their old defaults when the retired switch was on.
pub const MIN_QP: &[(&str, i64)] = &[
    ("nvenc_min_qp_h264", 19),
    ("nvenc_min_qp_hevc", 23),
    ("nvenc_min_qp_av1", 23),
];

/// App record keys Rubylight no longer reads.
pub const REMOVED_APP_KEYS: &[&str] = &["frame-gen-limiter-fix"];

/// App switches saved under an earlier name.
pub const RENAMED_APP_SWITCHES: &[(&str, &str)] = &[("virtual-screen", "virtual-display")];
