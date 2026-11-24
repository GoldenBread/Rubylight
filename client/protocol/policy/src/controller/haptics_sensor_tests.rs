//! Unit tests for `nightjar_policy::controller::haptics_sensor`.

use super::*;

#[test]
fn rumble_state_pause_resume_and_slot_release() {
    let mut state = RumbleState::new(true, RumbleFeel::Standard, 0);

    // Recording while unpaused is active.
    assert_eq!(
        state.on_rumble(0, 0x4000, 0x2000, false),
        RumbleRecordOutcome {
            allowed_to_record: true,
            active: true,
        }
    );

    // Pause returns slot 0.
    match state.set_paused(true, false) {
        PauseTransition::Pause { slots, len } => {
            assert_eq!(&slots[..len], &[0]);
        }
        other => panic!("expected Pause, got {other:?}"),
    }

    // Updates while paused still record latest levels, but report active=false.
    assert_eq!(
        state.on_rumble(0, 0, 0, false),
        RumbleRecordOutcome {
            allowed_to_record: true,
            active: false,
        }
    );
    assert_eq!(
        state.on_rumble_triggers(0, 0x1000, 0x2000, false),
        RumbleRecordOutcome {
            allowed_to_record: true,
            active: false,
        }
    );

    // Resume replays latest recorded values.
    match state.set_paused(false, false) {
        PauseTransition::Resume { entries, len } => {
            assert_eq!(&entries[..len], &[(0, 0, 0, 0x1000, 0x2000)]);
        }
        other => panic!("expected Resume, got {other:?}"),
    }

    // Releasing slot clears fallback vibrator and removes cached slot levels.
    assert_eq!(state.on_slot_released(0, true, false, true), (true, true));
    assert_eq!(
        state.set_paused(true, false),
        PauseTransition::Pause {
            slots: [0; MAX_GAMEPADS],
            len: 0
        }
    );

    // Disabled/stopped state rejects recording.
    let mut disabled = RumbleState::new(false, RumbleFeel::Standard, 0);
    assert_eq!(
        disabled.on_rumble(0, 0xFFFF, 0xFFFF, false),
        RumbleRecordOutcome {
            allowed_to_record: false,
            active: false,
        }
    );
}

#[test]
fn vibrator_amplitudes_and_device_routing() {
    assert_eq!(dual_vibrator_amplitudes(0, 0, RumbleFeel::Standard), None);
    assert_eq!(
        dual_vibrator_amplitudes(0x1200, 0xAB00, RumbleFeel::Standard),
        Some([0xAB, 0x12])
    );
    assert_eq!(
        quad_vibrator_amplitudes(0x1000, 0x2000, 0x3000, 0x4000, RumbleFeel::Standard),
        Some([0x20, 0x10, 0x30, 0x40])
    );
    assert_eq!(
        quad_vibrator_amplitudes(0, 0, 0, 0, RumbleFeel::Strong),
        None
    );

    let state = RumbleState::new(true, RumbleFeel::Standard, 1);
    assert!(matches!(
        state.device_vibrator_route(0, 0xFF00, 0, false, false, true, false, true, true, 50),
        DeviceVibratorAction::Osc { amplitude: 204, .. }
    ));
    assert!(matches!(
        state.device_vibrator_route(1, 0xFF00, 0, true, false, true, false, true, true, 50),
        DeviceVibratorAction::PlayerOneFallback { amplitude: 101, .. }
    ));
    assert_eq!(
        state.device_vibrator_route(1, 0xFF00, 0, true, true, true, false, true, true, 50),
        DeviceVibratorAction::None
    );
}

#[test]
fn gyro_liveness_rotation_rate_and_mapping() {
    let mut tracker = GyroLivenessTracker::new(10);
    assert!(!tracker.on_sample(0.0, 0.0, 0.0, 1));
    assert!(!tracker.on_sample(0.0, 0.0, 0.0, 10));
    assert!(tracker.on_sample(0.0, 0.0, 0.0, 11));
    assert!(!tracker.on_sample(0.0, 0.0, 0.0, 12));

    let mut real = GyroLivenessTracker::new(10);
    assert!(!real.on_sample(0.0, 0.0, 0.0, 1));
    assert!(!real.on_sample(1e-6, 0.0, 0.0, 5));
    assert!(!real.on_sample(0.0, 0.0, 0.0, 1000));

    let mut rewound = GyroLivenessTracker::new(10);
    assert!(!rewound.on_sample(0.0, 0.0, 0.0, 20));
    assert!(!rewound.on_sample(0.0, 0.0, 0.0, 15));
    assert!(!rewound.on_sample(0.0, 0.0, 0.0, 24));
    assert!(rewound.on_sample(0.0, 0.0, 0.0, 25));

    assert!(checks_gyro_liveness(1, 1, false, true, true));
    assert!(!checks_gyro_liveness(0, 1, false, true, true));
    assert!(!checks_gyro_liveness(1, 1, true, true, true));

    assert_eq!(clamp_sensor_rate_hz(500), 200);
    assert_eq!(clamp_sensor_rate_hz(100), 100);
    assert_eq!(sampling_period_us(200), 5000);
    assert_eq!(sampling_period_us(0), 0);
    assert_eq!(usb_motion_rate(500, true, false, true), 200);
    assert_eq!(usb_motion_rate(500, true, false, false), 500);
    assert_eq!(usb_motion_rate(500, false, false, false), 0);
    assert_eq!(usb_motion_rate(500, true, true, false), 0);

    let v = [1.0, 2.0, 3.0];
    assert_eq!(apply_device_rotation(v, 0), [1.0, 3.0, -2.0]);
    assert_eq!(apply_device_rotation(v, 1), [-2.0, 3.0, -1.0]);
    assert_eq!(apply_device_rotation(v, 2), [-1.0, 3.0, 2.0]);
    assert_eq!(apply_device_rotation(v, 3), [2.0, 3.0, 1.0]);

    assert_eq!(
        GyroMapping::validate([0, 0, 2], [1, 1, 1], 1.0, 1.0),
        Err(GyroMappingError::InvalidAxes)
    );
    assert_eq!(
        GyroMapping::validate([0, 1, 2], [1, 0, 1], 1.0, 1.0),
        Err(GyroMappingError::InvalidSigns)
    );
    assert_eq!(
        GyroMapping::validate([0, 1, 2], [1, 1, 1], 0.05, 1.0),
        Err(GyroMappingError::InvalidSensitivity)
    );
    assert_eq!(
        GyroMapping::FLYDIGI_TRANSFORM.transform([1.0, 2.0, 3.0]),
        [-2.0, 3.0, 1.0]
    );
}

#[test]
fn motion_filter_runs_liveness_before_dedup_and_scales_gyro() {
    let mut filter = MotionFilter::new(2, true, true, true, GyroMapping::IDENTITY);
    // Initial zero sample trips liveness at 1s even though dedup drops it from sending.
    let o1 = filter.on_sample([0.0, 0.0, 0.0], 5_000_000_000, 0, false);
    assert!(!o1.tripped_liveness && o1.send.is_none());
    let o2 = filter.on_sample([0.0, 0.0, 0.0], 5_999_999_999, 0, false);
    assert!(!o2.tripped_liveness && o2.send.is_none());
    let o3 = filter.on_sample([0.0, 0.0, 0.0], 6_000_000_000, 0, false);
    assert!(o3.tripped_liveness && o3.send.is_none());

    // Non-zero sample rotates (ROTATION_90: [-y, z, -x]), scales rad->deg, and applies Flydigi when forced.
    let o4 = filter.on_sample([1.0, 2.0, 3.0], 6_005_000_000, 1, true);
    let sent = o4.send.expect("expected sample");
    assert!((sent[0] - (-3.0 * RAD_TO_DEG)).abs() < 1e-4);
    assert!((sent[1] - (-RAD_TO_DEG)).abs() < 1e-4);
    assert!((sent[2] - (-2.0 * RAD_TO_DEG)).abs() < 1e-4);

    // Duplicate sample is dropped.
    let o5 = filter.on_sample([1.0, 2.0, 3.0], 6_010_000_000, 1, true);
    assert!(o5.send.is_none());
}

#[test]
fn battery_dedup_mapping_and_clamping() {
    assert!(are_battery_capacities_equal(f32::NAN, f32::NAN));
    assert!(!are_battery_capacities_equal(f32::NAN, 0.5));
    assert!(are_battery_capacities_equal(0.5, 0.5));

    assert_eq!(map_battery_status(1), Some(0));
    assert_eq!(map_battery_status(2), Some(3));
    assert_eq!(map_battery_status(3), Some(2));
    assert_eq!(map_battery_status(4), Some(4));
    assert_eq!(map_battery_status(5), Some(5));
    assert_eq!(map_battery_status(99), None);

    assert_eq!(battery_percentage_byte(f32::NAN), 0xFF);
    assert_eq!(battery_percentage_byte(0.5), 50);
    assert_eq!(battery_percentage_byte(1.5), 100);
    assert_eq!(battery_percentage_byte(-0.2), 0);

    assert_eq!(sce_battery_reading(100, 1, 2), (5, 1.0));
    assert_eq!(sce_battery_reading(50, 2, 1).0, 3);
    let (unk_status, unk_cap) = sce_battery_reading(-1, 0, 0);
    assert_eq!(unk_status, 1);
    assert!(unk_cap.is_nan());

    assert_eq!(evaluate_battery_update(false, 3, 0.5, 0, 0.0), None);
    assert_eq!(evaluate_battery_update(true, 3, 0.5, 3, 0.5), None);
    assert_eq!(
        evaluate_battery_update(true, 3, f32::NAN, 3, f32::NAN),
        None
    );
    assert_eq!(evaluate_battery_update(true, 3, 0.5, 0, 0.0), Some((2, 50)));
    assert_eq!(
        evaluate_battery_update(true, 1, f32::NAN, 0, 0.0),
        Some((0, 0xFF))
    );
}
