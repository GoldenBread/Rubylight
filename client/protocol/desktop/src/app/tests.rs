//! Tests for Windows swapchain colorspace selection, stream sleep pacing, and adaptive controller polling.

use super::*;

#[test]
fn windows_only_uses_scrgb_swapchain_for_hdr_and_10bit_sdr() {
    let default = config::Settings::default();
    assert!(!windows_needs_linear(&default), "default SDR stream uses 8-bit BGRA swapchain");
    // PyroWave, 4:4:4, packed 4:4:4 and upscaling in 8-bit SDR stay on 8-bit BGRA.
    let sdr_shaders = config::Settings {
        codec: config::CodecPreference::PyroWave,
        prefer_yuv444: true,
        experimental_packed_444: true,
        upscaling: config::Upscaling::Fsr1,
        ..default.clone()
    };
    assert!(!windows_needs_linear(&sdr_shaders));
    assert!(windows_needs_linear(&config::Settings { hdr: true, ..default.clone() }));
    assert!(windows_needs_linear(&config::Settings { sdr_10bit: true, ..default }));
}

#[test]
fn stream_sleep_avoids_busy_spin_except_for_beam_race() {
    // Normal sub-1.5 ms pacing tail uses kernel timer SDL_DelayNS, never SDL_DelayPrecise.
    assert_eq!(stream_sleep(Duration::from_micros(800), false, None), StreamSleep::DelayNs(800_000));
    assert_eq!(stream_sleep(Duration::from_nanos(200), false, None), StreamSleep::DelayNs(1_000), "positive sub-us clamped to 1 us");
    // Beam Race keeps microsecond spin precision.
    assert_eq!(stream_sleep(Duration::from_micros(800), true, None), StreamSleep::DelayPreciseNs(800_000));
    // Longer waits use SDL_WaitEventTimeout, waking 1 ms early for frames or at exact pad cadence.
    assert_eq!(stream_sleep(Duration::from_millis(8), false, None), StreamSleep::EventTimeoutMs(7));
    assert_eq!(stream_sleep(Duration::from_millis(8), false, Some(Duration::from_millis(2))), StreamSleep::EventTimeoutMs(2));
    assert_eq!(stream_sleep(Duration::from_millis(8), false, Some(Duration::from_millis(16))), StreamSleep::EventTimeoutMs(7));
}

#[test]
fn pad_poll_relaxes_idle_controllers_and_keeps_active_gameplay_fast() {
    let start = Instant::now();
    let mut poll = PadPoll::default();
    // First frame polls immediately so already-plugged pads open right away.
    assert_eq!(poll.next_in(start, false, false, false, true), Duration::ZERO);
    poll.polled(start);
    // No pads open -> 2 s hotplug interval.
    assert_eq!(poll.next_in(start, false, false, false, true), PadPoll::ENUM_INTERVAL);
    // Launcher with a controller open -> 16 ms visible, 250 ms hidden (not SDL's 1 ms).
    assert_eq!(poll.next_in(start, true, false, false, true), PadPoll::LAUNCHER_INTERVAL);
    assert_eq!(poll.next_in(start, true, false, false, false), PadPoll::LAUNCHER_HIDDEN_INTERVAL);
    // Stream with controllers disabled -> 2 s hotplug interval.
    assert_eq!(poll.next_in(start, true, true, false, true), PadPoll::ENUM_INTERVAL);
    // Stream with idle controller -> 16 ms.
    assert_eq!(poll.next_in(start, true, true, true, true), PadPoll::STREAM_IDLE_INTERVAL);
    // Controller input switches immediately to 2 ms active cadence for 1 s.
    poll.note_activity(start);
    assert_eq!(poll.next_in(start, true, true, true, true), PadPoll::STREAM_ACTIVE_INTERVAL);
    // DirectMotion filter activity also refreshes active cadence once per new timestamp.
    let later = start + Duration::from_secs(2);
    assert_eq!(poll.interval(later, true, true, true, true), PadPoll::STREAM_IDLE_INTERVAL);
    poll.sync_direct(42_000, later);
    assert_eq!(poll.interval(later, true, true, true, true), PadPoll::STREAM_ACTIVE_INTERVAL);
}

#[test]
fn ceil_ms_and_stream_sleep_never_truncate_sub_ms_pad_wait_to_zero() {
    assert_eq!(ceil_ms(Duration::ZERO), 0);
    assert_eq!(ceil_ms(Duration::from_nanos(1)), 1);
    assert_eq!(ceil_ms(Duration::from_micros(300)), 1);
    assert_eq!(ceil_ms(Duration::from_millis(1)), 1);
    assert_eq!(ceil_ms(Duration::from_micros(1_500)), 2);
    assert_eq!(ceil_ms(Duration::from_millis(16)), 16);
    assert_eq!(
        launcher_wait_slice_ms(Duration::from_millis(250), Duration::from_micros(200)),
        1,
        "positive sub-ms pad_wait rounds up to 1 ms instead of spinning at 0 ms"
    );
    assert_eq!(
        launcher_wait_slice_ms(Duration::from_millis(5), Duration::from_millis(16)),
        5
    );
    assert_eq!(
        stream_sleep(Duration::from_millis(8), false, Some(Duration::from_micros(200))),
        StreamSleep::EventTimeoutMs(1)
    );
    assert_eq!(
        stream_sleep(Duration::from_millis(8), false, Some(Duration::from_micros(1_500))),
        StreamSleep::EventTimeoutMs(2)
    );
}

#[test]
fn gamepad_activity_ignores_stick_jitter_and_passive_sensors() {
    assert!(is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_BUTTON_DOWN, None, None));
    assert!(is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION, None, None));
    assert!(is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_ADDED, None, None));
    assert!(!is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_AXIS_MOTION, Some(0), None));
    assert!(!is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_AXIS_MOTION, Some(1024), None));
    assert!(!is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_AXIS_MOTION, Some(-1024), None));
    assert!(is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_AXIS_MOTION, Some(1025), None));
    assert!(is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_AXIS_MOTION, Some(-12000), None));
    assert!(!is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_SENSOR_UPDATE, None, None), "accel ignored");
    assert!(!is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_SENSOR_UPDATE, None, Some(0.02)), "resting gyro ignored");
    assert!(!is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_SENSOR_UPDATE, None, Some(f32::NAN)));
    assert!(is_gamepad_activity_parts(SDL_EVENT_GAMEPAD_SENSOR_UPDATE, None, Some(0.5)), "active gyro wakes");

    // SAFETY: zero-initialized SDL_Event is valid plain data.
    let make_event = |kind: SDL_EventType| -> SDL_Event {
        let mut ev: SDL_Event = unsafe { std::mem::zeroed() };
        ev.r#type = kind.0;
        ev
    };
    for idle_kind in [
        SDL_EVENT_GAMEPAD_SENSOR_UPDATE,
        SDL_EVENT_SENSOR_UPDATE,
        SDL_EVENT_GAMEPAD_UPDATE_COMPLETE,
        SDL_EVENT_GAMEPAD_REMAPPED,
        SDL_EVENT_GAMEPAD_STEAM_HANDLE_UPDATED,
        SDL_EVENT_JOYSTICK_AXIS_MOTION,
        SDL_EVENT_JOYSTICK_BALL_MOTION,
        SDL_EVENT_JOYSTICK_HAT_MOTION,
        SDL_EVENT_JOYSTICK_BUTTON_DOWN,
        SDL_EVENT_JOYSTICK_BUTTON_UP,
        SDL_EVENT_JOYSTICK_ADDED,
        SDL_EVENT_JOYSTICK_REMOVED,
        SDL_EVENT_JOYSTICK_BATTERY_UPDATED,
        SDL_EVENT_JOYSTICK_UPDATE_COMPLETE,
    ] {
        let ev = make_event(idle_kind);
        assert!(!launcher_event_wakes_frame(&ev, false, [false; 4]), "idle launcher ignores {}", idle_kind.0);
        assert!(launcher_event_wakes_frame(&ev, true, [false; 4]), "controller test wakes on {}", idle_kind.0);
    }
    for wake_kind in [
        SDL_EVENT_GAMEPAD_ADDED,
        SDL_EVENT_GAMEPAD_REMOVED,
        SDL_EVENT_GAMEPAD_BUTTON_DOWN,
        SDL_EVENT_GAMEPAD_BUTTON_UP,
        SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN,
    ] {
        assert!(launcher_event_wakes_frame(&make_event(wake_kind), false, [false; 4]), "launcher wakes on {}", wake_kind.0);
    }

    let mut jitter = make_event(SDL_EVENT_GAMEPAD_AXIS_MOTION);
    jitter.gaxis.value = 300;
    assert!(!launcher_event_wakes_frame(&jitter, false, [false; 4]));
    jitter.gaxis.value = 4000;
    assert!(!launcher_event_wakes_frame(&jitter, false, [false; 4]));
    assert!(launcher_event_wakes_frame(&jitter, false, [true, false, false, false]));
    jitter.gaxis.value = 4001;
    assert!(launcher_event_wakes_frame(&jitter, false, [false; 4]));
}

#[test]
fn effective_launcher_window_mode_honors_windowed_launcher() {
    assert_eq!(effective_launcher_window_mode(WindowMode::Borderless, false), WindowMode::Borderless);
    assert_eq!(effective_launcher_window_mode(WindowMode::Fullscreen, false), WindowMode::Fullscreen);
    assert_eq!(effective_launcher_window_mode(WindowMode::Borderless, true), WindowMode::Windowed);
    assert_eq!(effective_launcher_window_mode(WindowMode::Fullscreen, true), WindowMode::Windowed);
    assert_eq!(effective_launcher_window_mode(WindowMode::Windowed, true), WindowMode::Windowed);
}

#[test]
fn v4l2_incomplete_base_probe_is_one_shot_not_sticky() {
    use nightjar_core::sdp::{VIDEO_FORMAT_AV1_MAIN8, VIDEO_FORMAT_H265};
    let wanted = VIDEO_FORMAT_H265;
    let key = (Hardware::V4l2m2m, wanted, false);
    let failed = stream::color_probe::Base {
        decodable: 0,
        reason: "HEVC: V4L2 M2M: the decoder made no picture of the fixture".into(),
        backends: Vec::new(),
    };
    assert!(!base_probe_sticky(key, &failed));
    let mut cache = Some(CachedBaseProbe { key, result: failed.clone(), sticky: false });
    // Peeking survives intermediate early returns (e.g. ProbeWork::Packed) on the same launch.
    assert_eq!(peek_base_probe(&cache, key), Some(failed.clone()));
    assert_eq!(peek_base_probe(&cache, key), Some(failed), "peek preserves non-sticky probe until launch commits");
    consume_non_sticky_base_probe(&mut cache, key);
    assert_eq!(peek_base_probe(&cache, key), None, "next launch re-probes V4L2 after commit");

    let passed = stream::color_probe::Base {
        decodable: wanted,
        reason: String::new(),
        backends: vec![(wanted, Hardware::V4l2m2m)],
    };
    assert!(base_probe_sticky(key, &passed));
    let mut sticky_cache = Some(CachedBaseProbe { key, result: passed.clone(), sticky: true });
    assert_eq!(peek_base_probe(&sticky_cache, key), Some(passed.clone()));
    consume_non_sticky_base_probe(&mut sticky_cache, key);
    assert_eq!(peek_base_probe(&sticky_cache, key), Some(passed), "full pass stays cached");

    let non_v4l2_key = (Hardware::Vaapi, VIDEO_FORMAT_AV1_MAIN8, false);
    let non_v4l2_fail = stream::color_probe::Base {
        decodable: 0,
        reason: "AV1: VA-API: no decoder".into(),
        backends: Vec::new(),
    };
    assert!(base_probe_sticky(non_v4l2_key, &non_v4l2_fail));
}

#[test]
fn pyrowave_probe_error_logs_once_per_distinct_reason() {
    let mut last = None;
    assert!(startup::should_log_pyrowave_probe_error(&mut last, false, "no Vulkan queue"));
    assert!(!startup::should_log_pyrowave_probe_error(&mut last, false, "no Vulkan queue"));
    assert!(startup::should_log_pyrowave_probe_error(&mut last, true, "no Vulkan queue"));
    assert!(startup::should_log_pyrowave_probe_error(&mut last, true, "shader compile failed"));
}
