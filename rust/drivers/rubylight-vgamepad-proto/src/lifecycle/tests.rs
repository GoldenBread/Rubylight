// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Unit tests for the virtual gamepad lifecycle state machine and VHF router.

use super::*;
use crate::profiles::pid_ff;
use crate::protocol::{FeedbackType, SteamHapticFeedback, sc_attr_flags};

#[test]
fn lifecycle_create_power_loss_recreate_and_counters() {
    let mut dev = DeviceState::new();
    dev.powered = true;
    let owner: OwnerToken = 101;
    let stranger: OwnerToken = 202;

    // Reserved profiles fail closed.
    for bad_prof in [Profile::GenericHid, Profile::GenericPid, Profile::Xbox360] {
        let req = CreateControllerRequest::new(0, bad_prof as u16);
        assert_eq!(
            dev.check_create_preflight(owner, &req),
            Err(DriverStatus::NotSupported)
        );
    }

    // Create DualSense on slot 0.
    let req = CreateControllerRequest::new(0, Profile::DualSense as u16);
    let (_, generation) = dev.check_create_preflight(owner, &req).unwrap();
    let plan = dev.begin_create(owner, &req, generation, false).unwrap();
    assert!(plan.register_feature_callbacks);
    dev.publish_vhf(0, 0xABCD);
    assert!(dev.commit_start(0, owner, true));
    assert_eq!(dev.slots[0].state, SlotState::Active);

    // Stranger cannot touch slot 0; query_controller_info rejects stranger.
    let id_req = ControllerIdRequest::new(0);
    assert_eq!(
        dev.query_controller_info(stranger, &id_req),
        Err(DriverStatus::AccessDenied)
    );
    assert_eq!(
        dev.poll_feedback(stranger, &id_req),
        Err(DriverStatus::AccessDenied)
    );
    assert_eq!(
        dev.poll_feedback(owner, &id_req),
        Err(DriverStatus::NoMoreEntries)
    );

    // D0 exit retires handle and marks slot Lost while preserving owner.
    let retired = dev.begin_power_down();
    assert_eq!(retired[0], Some(0xABCD));
    dev.finish_power_down(&retired);
    assert_eq!(dev.slots[0].state, SlotState::Lost);
    assert!(!dev.powered);
    assert_eq!(
        dev.poll_feedback(owner, &id_req),
        Err(DriverStatus::DeviceRemoved)
    );
    assert_eq!(
        dev.check_create_preflight(owner, &req),
        Err(DriverStatus::DevicePoweredOff)
    );

    // D0 entry restores power; stranger still blocked, owner recreates.
    dev.powered = true;
    assert_eq!(
        dev.check_create_preflight(stranger, &req),
        Err(DriverStatus::DeviceBusy)
    );
    let (_, gen2) = dev.check_create_preflight(owner, &req).unwrap();
    assert!(dev.begin_create(owner, &req, gen2, false).is_ok());
    dev.publish_vhf(0, 0xBEEF);
    assert!(dev.commit_start(0, owner, true));

    // Cleanup via owner destroy.
    let vhf = dev.begin_destroy_checked(owner, 0).unwrap();
    assert_eq!(vhf, Some(0xBEEF));
    dev.finish_destroy_owned(0, owner);
    assert_eq!(dev.slots[0].state, SlotState::Empty);
    assert_eq!(
        dev.begin_destroy_checked(owner, 0),
        Err(DriverStatus::NotFound)
    );
}

#[test]
fn steam_controller_settings_haptics_and_attributes_priority() {
    let mut dev = DeviceState::new();
    dev.powered = true;
    let owner = 55;
    let req = CreateControllerRequest::new(2, Profile::SteamController as u16);
    let (_, generation) = dev.check_create_preflight(owner, &req).unwrap();
    let _ = dev.begin_create(owner, &req, generation, false).unwrap();
    dev.publish_vhf(2, 0x2222);
    assert!(dev.commit_start(2, owner, true));

    // Forwarded real hardware attributes update feature replies, fail on non-SC26.
    let mut attr = SteamControllerAttributesRequest {
        controller_id: 2,
        flags: sc_attr_flags::FIRMWARE_BUILD_TIME | sc_attr_flags::SERIALS,
        firmware_build_time: 0x1122_3344,
        ..Default::default()
    };
    attr.unit_serial[..13].copy_from_slice(b"FXA9999999999");
    assert_eq!(dev.set_sc_attributes(owner, &attr), DriverStatus::Success);

    let mut get_buf = [0u8; 64];
    let mut get_unit = [0u8; 64];
    get_unit[0] = steam_controller::FEATURES_REPORT_ID;
    get_unit[1] = steam_controller::CMD_GET_STRING_ATTRIBUTE;
    get_unit[2] = 0x15;
    get_unit[3] = steam_controller::STRING_ATTR_UNIT_SERIAL;
    assert_eq!(
        dev.on_set_feature(2, steam_controller::FEATURES_REPORT_ID, &get_unit),
        DriverStatus::Success
    );
    assert_eq!(
        dev.on_get_feature(2, steam_controller::FEATURES_REPORT_ID, &mut get_buf)
            .unwrap(),
        64
    );
    assert_eq!(&get_buf[4..17], b"FXA9999999999");

    get_unit[3] = steam_controller::STRING_ATTR_BOARD_SERIAL;
    assert_eq!(
        dev.on_set_feature(2, steam_controller::FEATURES_REPORT_ID, &get_unit),
        DriverStatus::Success
    );
    assert_eq!(
        dev.on_get_feature(2, steam_controller::FEATURES_REPORT_ID, &mut get_buf)
            .unwrap(),
        64
    );
    assert_eq!(&get_buf[4..17], b"LVGSC26BOARD0");

    // Enqueue haptic + settings; settings must drain ahead of haptic.
    let haptic = [steam_controller::HAPTIC_PULSE_ID, 1, 0, 0, 0, 0, 0, 0];
    let (st, ack_sub, _) = dev.on_write_report(2, steam_controller::HAPTIC_PULSE_ID, &haptic, 100);
    assert_eq!(st, DriverStatus::Success);
    assert!(ack_sub.is_none()); // Not ready yet -> queued as Aside in pump

    let settings = [
        steam_controller::FEATURES_REPORT_ID,
        steam_controller::CMD_SET_SETTINGS_VALUES,
        3,
        9,
        0,
        0,
    ];
    assert_eq!(
        dev.on_set_feature(2, steam_controller::FEATURES_REPORT_ID, &settings),
        DriverStatus::Success
    );

    let id_req = ControllerIdRequest::new(2);
    let ev1 = dev.poll_feedback(owner, &id_req).unwrap();
    assert_eq!(ev1.feedback_type, FeedbackType::SteamSettings as u16);
    let ev2 = dev.poll_feedback(owner, &id_req).unwrap();
    assert_eq!(ev2.feedback_type, FeedbackType::SteamHaptic as u16);
    let parsed = SteamHapticFeedback::from_bytes(&ev2.payload).unwrap();
    assert_eq!(parsed.length, 8);
}

#[test]
fn cm_instance_list_fails_closed_on_malformed_and_matches_case_insensitively() {
    assert!(matches_cm_child_instance_list(&[], "RubylightGamepad0"));
    assert!(matches_cm_child_instance_list(
        &[b'a' as u16],
        "RubylightGamepad0"
    ));
    let mut list = [0u16; 48];
    for (dst, src) in list
        .iter_mut()
        .zip("HID\\VID_054C&PID_0CE6\\0&0000&RUBYLIGHTGAMEPAD3\0\0".encode_utf16())
    {
        *dst = src;
    }
    assert!(matches_cm_child_instance_list(&list, "RubylightGamepad3"));
    assert!(!matches_cm_child_instance_list(&list, "RubylightGamepad0"));
    assert!(!matches_cm_child_instance_list(&list, "RubylightGamepad30"));
}

#[test]
fn ntstatus_codes_and_keepalive_parking_and_pid_playing_match_contract() {
    assert_eq!(DriverStatus::Success.ntstatus(), 0x0000_0000u32 as i32);
    assert!(DriverStatus::Success.is_success());
    assert_eq!(
        DriverStatus::InvalidBufferSize.ntstatus(),
        0xC000_0206u32 as i32
    );
    assert_eq!(
        DriverStatus::DevicePoweredOff.ntstatus(),
        0x8000_000Fu32 as i32
    );
    assert_eq!(DriverStatus::DeviceBusy.ntstatus(), 0x8000_0011u32 as i32);
    assert_eq!(
        DriverStatus::NoMoreEntries.ntstatus(),
        0x8000_001Au32 as i32
    );
    for non_success in [
        DriverStatus::NotSupported,
        DriverStatus::InvalidParameter,
        DriverStatus::InvalidDeviceRequest,
        DriverStatus::AccessDenied,
        DriverStatus::BufferTooSmall,
        DriverStatus::InvalidBufferSize,
        DriverStatus::DeviceNotReady,
        DriverStatus::DevicePoweredOff,
        DriverStatus::DeviceBusy,
        DriverStatus::Cancelled,
        DriverStatus::NoMoreEntries,
        DriverStatus::NotFound,
        DriverStatus::DeviceRemoved,
    ] {
        assert!(!non_success.is_success(), "{non_success:?}");
    }

    // Steam Controller parks when nobody reads, unparks once on ready, and clears on destroy.
    let mut dev = DeviceState::new();
    dev.powered = true;
    let owner = 99;
    let create = CreateControllerRequest::new(0, Profile::SteamController as u16);
    let (_, gen_num) = dev.check_create_preflight(owner, &create).unwrap();
    let _ = dev.begin_create(owner, &create, gen_num, false).unwrap();
    dev.publish_vhf(0, 0xCAFE);
    assert!(dev.commit_start(0, owner, true));

    let input = InputStateRequest {
        controller_id: 0,
        ..Default::default()
    };
    let (pending, start) = dev.submit_input_state(owner, &input, 10_000, true).unwrap();
    assert!(pending.is_none());
    assert!(start);
    assert!(dev.sc26_timer_running);

    let (_, count, any_active, any_ready) = dev.sc26_keepalive_collect(20_000);
    assert_eq!(count, 0);
    assert!(any_active);
    assert!(!any_ready);
    assert!(dev.sc26_timer_parked);

    let (ready_pending, unpark) = dev.on_vhf_ready_for_next_report(0, 21_000);
    assert!(ready_pending.is_some());
    assert!(unpark);
    assert!(!dev.sc26_timer_parked);

    let (_, unpark_again) = dev.on_vhf_ready_for_next_report(0, 22_000);
    assert!(!unpark_again);

    assert!(!dev.any_pid_playing());
    dev.slots[0].force_feedback = true;
    assert!(dev.slots[0].pid.create_new_effect(&[
        pid_ff::PID_CREATE_NEW_EFFECT_REPORT_ID,
        1,
        0,
        0
    ]));
    assert!(
        dev.slots[0]
            .pid
            .effect_operation(&[pid_ff::PID_EFFECT_OPERATION_REPORT_ID, 1, 1, 1])
    );
    assert!(dev.any_pid_playing());
    dev.stopping = true;
    assert!(!dev.any_pid_playing());

    // Global `fill_query_info` aggregates per-slot feedback counters cleanly with saturation.
    let mut counter_dev = DeviceState::new();
    for _ in 0..(crate::steam_haptic_queue::HAPTIC_QUEUE_CAPACITY + 2) {
        counter_dev.slots[0]
            .haptic_queue
            .enqueue(crate::protocol::FeedbackEvent::default());
    }
    counter_dev.slots[1].sc26.features.unknown_commands = 3;
    let info = counter_dev.fill_query_info();
    assert_eq!(info.counters.haptic_evicted, 2);
    assert_eq!(info.counters.unknown_commands, 3);
}
