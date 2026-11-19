// Which tier each setting belongs to (the host console's All settings):
//
//   home         the Settings home (MAIN_SECTIONS in main.ts)
//   everyday     shown in its topic tab: every key not listed anywhere
//   expert       EXPERT_KEYS below: tuning and compatibility, shown in
//                Advanced > Expert settings, and in the topic tab only once changed
//   always-on    ALWAYS_ON in main.ts: on for everyone, in Advanced > Debug switches
//   diagnostic   DIAGNOSTICS in main.ts: troubleshooting, also in Debug switches
//   games-page   GAMES_PAGE_KEYS below: a launcher's options, which the Games page links to
//
// A setting whose value differs from its default also shows in its topic tab
// (settings-schema.ts tabLayout), so nothing a user changed is hidden. The
// comment above each run of keys names the tab and group the keys come from.
import { ALWAYS_ON, DIAGNOSTICS, MAIN_KEYS } from './main';

/** Tuning and compatibility: the defaults are the measured ones. */
export const EXPERT_KEYS: readonly string[] = [
  // This PC › Name and language
  'locale',
  // This PC › Tray
  'hide_tray_controls',
  // This PC › Updates
  'notify_pre_releases',
  // This PC › Hardware metrics
  'hardware_metrics', 'hardware_metrics_interval', 'hardware_metrics_selection',
  // Picture › Capture
  'capture', 'capture_window_exe', 'capture_window_title',
  // Picture › Codecs
  'hevc_mode', 'av1_mode', 'pyrowave_encode_overlap',
  // Picture › HDR
  'hdr_dynamic_metadata', 'prefer_sdr_10bit', 'vulkan_hdr_layer', 'virtual_display_hdr_calibration',
  // Picture › Frame pacing
  'frame_pacing', 'minimum_fps_target', 'limit_framerate', 'phase_lock_hold_at_stream_rate',
  // Picture › Bitrate and network
  'fec_percentage', 'adaptive_fec', 'fec_max_percentage', 'pyrowave_critical_fec_percentage',
  'video_max_batch_size_kb', 'pacing_max_bitrate_kbps', 'host_adaptive_pacing', 'congestion_control',
  // Picture › Encoder
  'adapter_pnp_id',
  // Picture › AMD AMF
  'amd_usage', 'amd_quality', 'amd_rc', 'amd_peak_bitrate_ratio', 'amd_vbv_buffer_frames', 'amd_max_frame_size',
  'amd_qvbr_quality_level', 'amd_preanalysis', 'amd_vbaq', 'amd_enforce_hrd', 'amd_coder', 'amd_input_queue_size',
  'amd_ltr_frames', 'amd_lowlatency_mode', 'amd_high_motion_quality_boost', 'amd_vpe', 'amd_smart_access_video',
  'amd_split_frame', 'amd_av1_latency_mode', 'amd_av1_screen_content', 'amd_av1_tiles',
  // Picture › NVIDIA NVENC
  'nvenc_preset', 'nvenc_rate_control', 'nvenc_target_quality', 'nvenc_peak_bitrate_ratio', 'nvenc_twopass',
  'nvenc_spatial_aq', 'nvenc_temporal_aq', 'nvenc_split_encode', 'nvenc_vbv_increase', 'nvenc_weighted_prediction',
  'nvenc_h264_cavlc', 'nvenc_insert_filler_data', 'nvenc_enable_min_qp', 'nvenc_min_qp_h264', 'nvenc_min_qp_hevc',
  'nvenc_min_qp_av1', 'nvenc_latency_over_power',
  // Picture › Streamed display
  'dd_activate_virtual_display', 'dd_rotation_option', 'dd_manual_rotation',
  // Picture › Mode substitutions
  'dd_mode_remapping',
  // Picture › After a stream
  'dd_config_revert_delay', 'dd_always_restore_from_golden', 'dd_snapshot_exclude_devices',
  'dd_snapshot_restore_hotkey', 'dd_snapshot_restore_hotkey_modifiers',
  // Picture › Frame limiter
  'virtual_display_refresh',
  // Picture › RTSS
  'rtss_install_path', 'rtss_frame_limit_type', 'rtss_allow_virtual_display_override',
  // Picture › Resolution and refresh rate
  'fallback_mode', 'max_stream_resolution',
  // Sound & controls › Audio devices
  'audio_sink_capture_only', 'auto_capture_sink',
  // Sound & controls › Virtual speakers
  'keep_sink_default', 'hold_sink_while_paused', 'install_steam_audio_drivers',
  // Sound & controls › Input devices
  'enable_input_only_mode',
  // Sound & controls › Keyboard
  'always_send_scancodes', 'key_rightalt_to_key_win', 'key_repeat_delay', 'key_repeat_frequency',
  'keyboard_follows_focus', 'text_input_surrounding_text',
  // Sound & controls › Key remapping
  'keybindings',
  // Sound & controls › Mouse, pen and touch
  'high_resolution_scrolling', 'native_pen_touch', 'mouse_mode', 'native_touchpad', 'pen_barrel_roll',
  'touch_sensitivity', 'touch_gestures', 'client_cursor',
  // Sound & controls › Controllers
  'motion_as_ds4', 'touchpad_as_ds4', 'ds4_back_as_touchpad_click', 'dsu_server_port', 'dsu_server_lan',
  'audio_haptics_curve', 'audio_haptics_gate', 'dualsense_haptics_gain', 'back_button_timeout',
  // Games › App behavior
  'remote_monitor_terminate_on_first_request', 'exit_timeout',
  // Games › Playnite
  'playnite_sync_plugins', 'playnite_exclude_categories', 'playnite_exclude_plugins',
  'playnite_autosync_require_replacement', 'playnite_exit_wait_secs',
  // Games › Playnite launch focus
  'playnite_focus_attempts', 'playnite_focus_timeout_secs', 'playnite_focus_exit_on_first',
  // Games › Lossless Scaling
  'lossless_scaling_legacy_auto_detect',
  // Network › Ports and addresses
  'address_family', 'bind_address',
  // Network › Discovery and internet
  'enable_discovery', 'internet_play_stun_servers',
  // Network › WebRTC
  'webrtc_port', 'webrtc_media_port',
  // Network › File transfer
  'file_transfer_max_mb', 'file_transfer_stream_mbps',
  // Network › Clipboard
  'clipboard_image_max_mb',
  // Network › Streams
  'ping_timeout', 'dscp_tagging',
  // Advanced › Logging
  'min_log_level', 'log_path',
  // Advanced › Compatibility
  'legacy_ordering', 'envvar_compatibility_mode',
  // Advanced › Console
  'csrf_allowed_origins', 'session_token_ttl_seconds', 'remember_me_refresh_token_ttl_seconds',
];

/** The launchers' option groups: the Games page links to each, so the Games tab keeps them out of the way. */
export const GAMES_PAGE_KEYS: readonly string[] = [
  // Games › Steam options
  'steam_enabled', 'steam_auto_sync', 'steam_sync_all_installed', 'steam_recent_games', 'steam_recent_max_age_days',
  'steam_autosync_remove_uninstalled', 'steam_include_tools', 'steam_exclude_games',
  // Games › Epic Games options
  'library_epic_enabled', 'epic_auto_sync', 'epic_autosync_remove_uninstalled', 'epic_exclude_games',
  // Games › Battle.net options
  'library_battlenet_enabled', 'battlenet_auto_sync', 'battlenet_sync_all_installed', 'battlenet_recent_games',
  'battlenet_recent_max_age_days', 'battlenet_autosync_remove_uninstalled', 'battlenet_exclude_games',
  // Games › Microsoft Store and Xbox options
  'library_microsoft_enabled', 'microsoft_auto_sync', 'microsoft_autosync_remove_uninstalled',
  'microsoft_exclude_games',
  // Games › EA app options
  'library_ea_enabled', 'ea_auto_sync', 'ea_autosync_remove_uninstalled', 'ea_exclude_games',
  // Games › Ubisoft Connect options
  'library_ubisoft_enabled', 'ubisoft_auto_sync', 'ubisoft_autosync_remove_uninstalled', 'ubisoft_exclude_games',
  // Games › GOG options
  'library_gog_enabled', 'gog_auto_sync', 'gog_autosync_remove_uninstalled', 'gog_exclude_games',
];

export const EXPERT_KEY_SET: ReadonlySet<string> = new Set(EXPERT_KEYS);
export const GAMES_PAGE_KEY_SET: ReadonlySet<string> = new Set(GAMES_PAGE_KEYS);
const HOME_KEY_SET: ReadonlySet<string> = new Set(MAIN_KEYS);

export type Tier = 'home' | 'everyday' | 'expert' | 'always-on' | 'diagnostic' | 'games-page';

/** The tier of a setting by its key. */
export function tierOf(key: string): Tier {
  if (HOME_KEY_SET.has(key)) return 'home';
  if (EXPERT_KEY_SET.has(key)) return 'expert';
  if (ALWAYS_ON.includes(key)) return 'always-on';
  if (DIAGNOSTICS.includes(key)) return 'diagnostic';
  if (GAMES_PAGE_KEY_SET.has(key)) return 'games-page';
  return 'everyday';
}

/** Shown in its topic tab only once changed (or, for a launcher group, when asked for). */
export const isEveryday = (key: string): boolean => {
  const tier = tierOf(key);
  return tier === 'home' || tier === 'everyday';
};
