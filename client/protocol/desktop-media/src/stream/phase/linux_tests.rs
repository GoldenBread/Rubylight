//! Unit tests for Linux display timing, EGL SBC tracking, DRM/EGL period alignment, and
//! source-switch hysteresis.

use super::*;

#[test]
fn monotonic_ust_accepts_microseconds_and_nanoseconds_and_rejects_bad_epochs() {
    let now_ns: i128 = 10_000_000_000_000; // 10,000 s uptime
    let now_us = (now_ns / 1_000) as u64;
    // Mesa DRI3 returns microseconds.
    assert_eq!(monotonic_ust_age_us(now_ns, now_us - 4_200), Some(4_200));
    assert_eq!(monotonic_ust_age_us(now_ns, now_us), Some(0));
    // Slight future projection (<= 5 ms) accepted.
    assert_eq!(monotonic_ust_age_us(now_ns, now_us + 2_000), Some(-2_000));
    // ANGLE/nanosecond implementations return nanoseconds.
    assert_eq!(monotonic_ust_age_us(now_ns, (now_ns - 6_500_000) as u64), Some(6_500));
    assert_eq!(monotonic_ust_age_us(now_ns, (now_ns + 1_000_000) as u64), Some(-1_000));
    // Zero, stale (> 100 ms), far future (> 5 ms), and CLOCK_REALTIME epochs are rejected.
    assert_eq!(monotonic_ust_age_us(now_ns, 0), None);
    assert_eq!(monotonic_ust_age_us(now_ns, now_us - 100_001), None);
    assert_eq!(monotonic_ust_age_us(now_ns, now_us + 5_001), None);
    assert_eq!(monotonic_ust_age_us(now_ns, 1_750_000_000_000_000), None);
    // EGL DRI3 extended grace window tolerates brief <=500 ms pauses without flapping to DRM.
    assert_eq!(
        monotonic_ust_age_within_us(now_ns, now_us - 350_000, EGL_STALE_GRACE_US),
        Some(350_000)
    );
    assert_eq!(
        monotonic_ust_age_within_us(now_ns, now_us - 500_001, EGL_STALE_GRACE_US),
        None
    );
}

#[test]
fn monotonic_ns_validates_drm_vblank_timestamps() {
    let now_ns: i128 = 50_000_000_000;
    assert_eq!(monotonic_ns_age_us(now_ns, (now_ns - 3_400_000) as i64), Some(3_400));
    assert_eq!(monotonic_ns_age_us(now_ns, (now_ns + 1_000_000) as i64), Some(-1_000));
    assert_eq!(monotonic_ns_age_us(now_ns, 0), None);
    assert_eq!(monotonic_ns_age_us(now_ns, -1), None);
    assert_eq!(monotonic_ns_age_us(now_ns, (now_ns - 150_000_000) as i64), None);
}

#[test]
fn period_selection_keeps_matching_measured_rate_and_rejects_mismatched_60hz_fallback() {
    // Steam Deck OLED 89.89 Hz: exact kernel/EGL period is kept.
    let deck_ns = period_ns_from_hz(89.89).unwrap();
    assert_eq!(deck_ns, 11_124_708);
    assert_eq!(choose_period_ns(Some(11_125_000), 89.89), Some(11_125_000));
    // A synthetic 60 Hz Xwayland rate (16.67 ms) on an 89.89 Hz panel is rejected (>5% off).
    assert_eq!(choose_period_ns(rate_period_ns(60, 1), 89.89), Some(deck_ns));
    // Missing measured rate falls back to SDL refresh_hz.
    assert_eq!(choose_period_ns(None, 89.89), Some(deck_ns));
    // Without refresh_hz, a sane measured rate still works.
    assert_eq!(choose_period_ns(rate_period_ns(90_000, 1_001), 0.0), Some(11_122_222));
    // Out-of-range rates are rejected.
    assert_eq!(rate_period_ns(0, 1), None);
    assert_eq!(rate_period_ns(10, 1), None);
    assert_eq!(period_ns_from_hz(0.0), None);
    assert_eq!(period_ns_from_hz(f32::NAN), None);
}

#[test]
fn drm_period_aligns_with_exact_sdl_panel_rate_to_prevent_egl_drm_mismatch() {
    // Nightjar #175: kernel nominal 90.00 Hz vs exact Steam Deck OLED 89.89 Hz agree within 2%,
    // so DRM vblank reports the same 11.125 ms period as EGL sync instead of 11.111 ms.
    let deck_ns = period_ns_from_hz(89.89).unwrap();
    assert_eq!(drm_period_ns(90.0, 89.89), Some(deck_ns));
    assert_eq!(drm_period_ns(60.0, 59.94), period_ns_from_hz(59.94));
    // Without SDL rate, falls back to DRM mode rate.
    assert_eq!(drm_period_ns(90.0, 0.0), period_ns_from_hz(90.0));
}

#[test]
fn source_hysteresis_prevents_drm_egl_flapping_after_dropout() {
    let mut h = SourceHysteresis::default();
    let egl = ((1_000, 11_124_708), TimingSource::EglSync);
    let drm = ((1_000, 11_124_708), TimingSource::DrmVblank);
    // Starts on preferred EGL.
    assert_eq!(h.pick(Some(egl), None).map(|(_, s)| s), Some(TimingSource::EglSync));
    // Dropout >500 ms -> EGL unavailable -> switches immediately to DRM vblank.
    assert_eq!(h.pick(None, Some(drm)).map(|(_, s)| s), Some(TimingSource::DrmVblank));
    // Single intermittent EGL sample does NOT flap back from DRM.
    assert_eq!(h.pick(Some(egl), Some(drm)).map(|(_, s)| s), Some(TimingSource::DrmVblank));
    assert_eq!(h.pick(None, Some(drm)).map(|(_, s)| s), Some(TimingSource::DrmVblank));
    // Sustained healthy EGL samples switch cleanly back after SOURCE_SWITCH_CONFIRM.
    for _ in 1..SOURCE_SWITCH_CONFIRM {
        assert_eq!(h.pick(Some(egl), Some(drm)).map(|(_, s)| s), Some(TimingSource::DrmVblank));
    }
    assert_eq!(h.pick(Some(egl), Some(drm)).map(|(_, s)| s), Some(TimingSource::EglSync));
}

#[test]
fn extension_check_matches_whole_tokens_only() {
    let exts = "EGL_KHR_image EGL_CHROMIUM_sync_control EGL_ANGLE_sync_control_rate";
    assert!(has_extension(exts, "EGL_CHROMIUM_sync_control"));
    assert!(has_extension(exts, "EGL_ANGLE_sync_control_rate"));
    assert!(!has_extension(exts, "EGL_CHROMIUM_sync"));
    assert!(!has_extension("EGL_CHROMIUM_sync_control_extra", "EGL_CHROMIUM_sync_control"));
}

#[test]
fn glass_phase_timing_supports_one_to_one_wayland_and_kms_streams() {
    assert_eq!(
        glass_phase_timing(Some(500_000), Some(11_125_000), 504_000, 89.89),
        Some((500_000, 11_125_000))
    );
    // Compositor reporting 0 refresh falls back to display refresh_hz.
    assert_eq!(
        glass_phase_timing(Some(500_000), None, 504_000, 89.89),
        Some((500_000, 11_124_708))
    );
    // Stale glass timestamp (>100 ms old) returns None.
    assert_eq!(glass_phase_timing(Some(500_000), None, 600_001, 89.89), None);
    assert_eq!(glass_phase_timing(None, None, 504_000, 89.89), None);
}

#[test]
fn egl_swap_tracker_verifies_sbc_before_waiting_and_handles_stuck_drivers() {
    let mut tracker = EglSwapTracker::default();
    tracker.bind(0x1000);
    // Before verification, never waits/refuses.
    assert_eq!(tracker.should_wait(40), Some(false));
    tracker.note_presented(40);
    // Completed SBC advanced to 41 -> verified, swapchain clear.
    assert_eq!(tracker.should_wait(41), Some(false));
    tracker.note_presented(41);
    // Still at 41 -> swap 42 is in flight -> wait/refuse!
    assert_eq!(tracker.should_wait(41), Some(true));
    // Completed SBC catches up to 42 -> clear to present.
    assert_eq!(tracker.should_wait(42), Some(false));
    // External redraw advanced SBC beyond submitted -> resyncs cleanly without waiting.
    assert_eq!(tracker.should_wait(45), Some(false));
    tracker.note_presented(45);
    assert_eq!(tracker.should_wait(45), Some(true));
    // After an external waiting present, clears pending watermark so next probe does not stall.
    tracker.note_external_present();
    assert_eq!(tracker.should_wait(45), Some(false));
    // Changing surface resets state.
    tracker.bind(0x2000);
    assert!(!tracker.verified);

    // A broken driver whose SBC never increments falls back to None after UNVERIFIED_LIMIT.
    let mut stuck = EglSwapTracker::default();
    stuck.bind(0x3000);
    for _ in 0..UNVERIFIED_LIMIT {
        assert_eq!(stuck.should_wait(0), Some(false));
        stuck.note_presented(0);
    }
    assert_eq!(stuck.should_wait(0), None);
}
