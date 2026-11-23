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
