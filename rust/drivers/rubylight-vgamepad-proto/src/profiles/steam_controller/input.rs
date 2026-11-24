// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad include/libvirtualgamepad/sc26_usb.h and
// driver/src/steam_controller.h / steam_controller.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Steam Controller (2026) input, battery, touch, and IMU report encoding.

use super::{
    ACCEL_COUNTS_PER_G, BATTERY_REPORT_ID, BATTERY_REPORT_SIZE, CHARGE_CHARGING,
    CHARGE_DISCHARGING, CHARGE_DONE, GYRO_COUNTS_PER_1000_DPS, INPUT_REPORT_ID, INPUT_REPORT_SIZE,
    MILLI_G, STICK_TOUCH_DEFLECTION, Sc26DeviceState, Sc26State, TRIGGER_CLICK_THRESHOLD, sc26_btn,
};
use crate::protocol::{
    BatteryState, BatteryStateRequest, InputStateRequest, MotionKind, MotionStateRequest,
    TouchEvent, TouchStateRequest, button_mask,
};

const fn clamp_i16(value: i64) -> i16 {
    if value > 32767 {
        32767
    } else if value < -32768 {
        -32768
    } else {
        value as i16
    }
}

const fn pad_axis_x(x: u16) -> i16 {
    clamp_i16((x as i64) - 32768)
}

const fn pad_axis_y(y: u16) -> i16 {
    clamp_i16(32768 - (y as i64))
}

const fn trigger_axis(trigger: u8) -> i16 {
    (((trigger as i32) * 32767) / 255) as i16
}

pub fn sc26_tick(state: &mut Sc26State, now_us: u64) {
    state.device.imu_timestamp = now_us as u32;
    if !state.grip_explicit {
        state.device.grip_touch = [false; 2];
    }
}

pub fn sc26_buttons(buttons: u32, state: &Sc26State) -> u32 {
    const MAP: [(u32, u32); 20] = [
        (button_mask::SOUTH, sc26_btn::A),
        (button_mask::EAST, sc26_btn::B),
        (button_mask::WEST, sc26_btn::X),
        (button_mask::NORTH, sc26_btn::Y),
        (button_mask::DPAD_UP, sc26_btn::DPAD_UP),
        (button_mask::DPAD_DOWN, sc26_btn::DPAD_DOWN),
        (button_mask::DPAD_LEFT, sc26_btn::DPAD_LEFT),
        (button_mask::DPAD_RIGHT, sc26_btn::DPAD_RIGHT),
        (button_mask::START, sc26_btn::VIEW),
        (button_mask::BACK, sc26_btn::MENU),
        (button_mask::LEFT_STICK, sc26_btn::L3),
        (button_mask::RIGHT_STICK, sc26_btn::R3),
        (button_mask::LEFT_SHOULDER, sc26_btn::L),
        (button_mask::RIGHT_SHOULDER, sc26_btn::R),
        (button_mask::HOME, sc26_btn::STEAM),
        (button_mask::MISC, sc26_btn::QAM),
        (button_mask::PADDLE_1, sc26_btn::R4),
        (button_mask::PADDLE_2, sc26_btn::L4),
        (button_mask::PADDLE_3, sc26_btn::R5),
        (button_mask::PADDLE_4, sc26_btn::L5),
    ];

    let mut out = 0u32;
    for &(from, to) in &MAP {
        if (buttons & from) != 0 {
            out |= to;
        }
    }
    if (buttons & button_mask::SC26_BUTTONS_EXPLICIT) != 0 {
        if (buttons & button_mask::LEFT_PAD_CLICK) != 0 {
            out |= sc26_btn::LEFT_PAD_CLICK;
        }
        if (buttons & button_mask::RIGHT_PAD_CLICK) != 0 {
            out |= sc26_btn::RIGHT_PAD_CLICK;
        }
        if (buttons & button_mask::LEFT_TRIGGER_CLICK) != 0 {
            out |= sc26_btn::LEFT_TRIGGER_CLICK;
        }
        if (buttons & button_mask::RIGHT_TRIGGER_CLICK) != 0 {
            out |= sc26_btn::RIGHT_TRIGGER_CLICK;
        }
    } else if (buttons & button_mask::TOUCHPAD) != 0 {
        let left = state.device.pad_touched[0];
        let right = state.device.pad_touched[1];
        if left {
            out |= sc26_btn::LEFT_PAD_CLICK;
        }
        if right || !left {
            out |= sc26_btn::RIGHT_PAD_CLICK;
        }
    }
    out
}

pub const fn classify_buttons(st: &Sc26DeviceState, mut buttons: u32) -> u32 {
    if st.stick_touch_from_deflection {
        buttons &= !(sc26_btn::LEFT_STICK_TOUCH | sc26_btn::RIGHT_STICK_TOUCH);
    }
    buttons
}

pub fn encode_input_with_buttons(
    buttons: u32,
    input: &InputStateRequest,
    st: &mut Sc26DeviceState,
) -> [u8; INPUT_REPORT_SIZE] {
    let mut r = [0u8; INPUT_REPORT_SIZE];
    r[0] = INPUT_REPORT_ID;
    r[1] = st.sequence;
    st.sequence = st.sequence.wrapping_add(1);

    let mut bits = buttons;
    if st.trigger_click_from_analog {
        if input.left_trigger >= TRIGGER_CLICK_THRESHOLD {
            bits |= sc26_btn::LEFT_TRIGGER_CLICK;
        }
        if input.right_trigger >= TRIGGER_CLICK_THRESHOLD {
            bits |= sc26_btn::RIGHT_TRIGGER_CLICK;
        }
    }
    if st.pad_touched[0] {
        bits |= sc26_btn::LEFT_PAD_TOUCH;
    }
    if st.pad_touched[1] {
        bits |= sc26_btn::RIGHT_PAD_TOUCH;
    }
    if st.grip_touch[0] {
        bits |= sc26_btn::LEFT_GRIP_TOUCH;
    }
    if st.grip_touch[1] {
        bits |= sc26_btn::RIGHT_GRIP_TOUCH;
    }
    if st.stick_touch[0] {
        bits |= sc26_btn::LEFT_STICK_TOUCH;
    }
    if st.stick_touch[1] {
        bits |= sc26_btn::RIGHT_STICK_TOUCH;
    }
    if st.stick_touch_from_deflection {
        let deflected = |x: i16, y: i16| -> bool {
            i32::from(x).abs() > STICK_TOUCH_DEFLECTION
                || i32::from(y).abs() > STICK_TOUCH_DEFLECTION
        };
        if deflected(input.left_x, input.left_y) {
            bits |= sc26_btn::LEFT_STICK_TOUCH;
        }
        if deflected(input.right_x, input.right_y) {
            bits |= sc26_btn::RIGHT_STICK_TOUCH;
        }
    }

    r[2..6].copy_from_slice(&bits.to_le_bytes());
    r[6..8].copy_from_slice(&trigger_axis(input.left_trigger).to_le_bytes());
    r[8..10].copy_from_slice(&trigger_axis(input.right_trigger).to_le_bytes());
    r[10..12].copy_from_slice(&input.left_x.to_le_bytes());
    r[12..14].copy_from_slice(&input.left_y.to_le_bytes());
    r[14..16].copy_from_slice(&input.right_x.to_le_bytes());
    r[16..18].copy_from_slice(&input.right_y.to_le_bytes());

    if st.pad_touched[0] {
        r[18..20].copy_from_slice(&pad_axis_x(st.pad_x[0]).to_le_bytes());
        r[20..22].copy_from_slice(&pad_axis_y(st.pad_y[0]).to_le_bytes());
        r[22..24].copy_from_slice(&(st.pad_pressure[0] >> 1).to_le_bytes());
    }
    if st.pad_touched[1] {
        r[24..26].copy_from_slice(&pad_axis_x(st.pad_x[1]).to_le_bytes());
        r[26..28].copy_from_slice(&pad_axis_y(st.pad_y[1]).to_le_bytes());
        r[28..30].copy_from_slice(&(st.pad_pressure[1] >> 1).to_le_bytes());
    }

    r[30..34].copy_from_slice(&st.imu_timestamp.to_le_bytes());
    r[34..36].copy_from_slice(&st.accel[0].to_le_bytes());
    r[36..38].copy_from_slice(&st.accel[1].to_le_bytes());
    r[38..40].copy_from_slice(&st.accel[2].to_le_bytes());
    r[40..42].copy_from_slice(&st.gyro[0].to_le_bytes());
    r[42..44].copy_from_slice(&st.gyro[1].to_le_bytes());
    r[44..46].copy_from_slice(&st.gyro[2].to_le_bytes());
    r[46..48].copy_from_slice(&32767i16.to_le_bytes()); // identity quat_w
    r
}

pub fn encode_input(input: &InputStateRequest, state: &mut Sc26State) -> [u8; INPUT_REPORT_SIZE] {
    let explicit_buttons = (input.buttons & button_mask::SC26_BUTTONS_EXPLICIT) != 0;
    if explicit_buttons {
        state.trigger_click_explicit = true;
        state.grip_explicit = true;
        state.stick_touch_explicit = true;
        state.device.stick_touch_from_deflection = false;
    }
    state.device.trigger_click_from_analog = !state.trigger_click_explicit;
    let grip_bits = button_mask::LEFT_GRIP_TOUCH | button_mask::RIGHT_GRIP_TOUCH;
    if (input.buttons & grip_bits) != 0 {
        state.grip_explicit = true;
    }
    if state.grip_explicit {
        state.device.grip_touch[0] = (input.buttons & button_mask::LEFT_GRIP_TOUCH) != 0;
        state.device.grip_touch[1] = (input.buttons & button_mask::RIGHT_GRIP_TOUCH) != 0;
    }
    let stick_touch_bits = button_mask::LEFT_STICK_TOUCH | button_mask::RIGHT_STICK_TOUCH;
    if (input.buttons & stick_touch_bits) != 0 {
        state.stick_touch_explicit = true;
        state.device.stick_touch_from_deflection = false;
    }
    if state.stick_touch_explicit {
        state.device.stick_touch[0] = (input.buttons & button_mask::LEFT_STICK_TOUCH) != 0;
        state.device.stick_touch[1] = (input.buttons & button_mask::RIGHT_STICK_TOUCH) != 0;
    }

    let btns = sc26_buttons(input.buttons, state);
    encode_input_with_buttons(btns, input, &mut state.device)
}

pub fn encode_battery(state: &Sc26State) -> [u8; BATTERY_REPORT_SIZE] {
    let st = &state.device;
    let mut r = [0u8; BATTERY_REPORT_SIZE];
    let level = st.battery_percent.min(100);
    let on_usb = st.charge_state == CHARGE_CHARGING || st.charge_state == CHARGE_DONE;
    let vbat = 3522u16 + u16::from(level) * 6;
    let vin = if on_usb { 4980u16 } else { 0 };
    let curr = if on_usb { 157u16 } else { 0 };
    let iin = if on_usb { 239u16 } else { 0 };

    r[0] = BATTERY_REPORT_ID;
    r[1] = st.charge_state;
    r[2] = level;
    r[3..5].copy_from_slice(&vbat.to_le_bytes());
    r[5..7].copy_from_slice(&4160u16.to_le_bytes());
    r[7..9].copy_from_slice(&vin.to_le_bytes());
    r[9..11].copy_from_slice(&curr.to_le_bytes());
    r[11..13].copy_from_slice(&iin.to_le_bytes());
    r[13..15].copy_from_slice(&0x76EDu16.to_le_bytes());
    r
}

pub fn apply_touch(touch: &TouchStateRequest, state: &mut Sc26State) -> bool {
    let Some(event) = TouchEvent::from_raw(touch.event_type) else {
        return false;
    };
    let st = &mut state.device;
    if event == TouchEvent::CancelAll {
        st.pad_touched = [false; 2];
        return true;
    }
    if touch.contact_index >= 2 {
        return false;
    }
    let pad = usize::from(touch.contact_index);
    match event {
        TouchEvent::Hover | TouchEvent::Move => {
            if !st.pad_touched[pad] {
                return false;
            }
            st.pad_x[pad] = touch.x;
            st.pad_y[pad] = touch.y;
            st.pad_pressure[pad] = touch.pressure;
            true
        }
        TouchEvent::Down => {
            st.pad_touched[pad] = true;
            st.pad_x[pad] = touch.x;
            st.pad_y[pad] = touch.y;
            st.pad_pressure[pad] = touch.pressure;
            true
        }
        TouchEvent::Up | TouchEvent::Cancel => {
            st.pad_touched[pad] = false;
            st.pad_pressure[pad] = 0;
            true
        }
        TouchEvent::CancelAll => true,
    }
}

pub fn apply_accel_milli(st: &mut Sc26DeviceState, x: i32, y: i32, z: i32) {
    let convert = |milli: i64| clamp_i16((milli * ACCEL_COUNTS_PER_G) / MILLI_G);
    st.accel[0] = convert(i64::from(x));
    st.accel[2] = convert(i64::from(y));
    st.accel[1] = convert(-i64::from(z));
}

pub fn apply_gyro_milli(st: &mut Sc26DeviceState, x: i32, y: i32, z: i32) {
    let convert = |milli: i64| clamp_i16((milli * GYRO_COUNTS_PER_1000_DPS) / 1_000_000);
    st.gyro[0] = convert(i64::from(x));
    st.gyro[2] = convert(i64::from(y));
    st.gyro[1] = convert(-i64::from(z));
}

pub fn apply_motion(motion: &MotionStateRequest, state: &mut Sc26State, now_us: u64) -> bool {
    match MotionKind::from_raw(motion.motion_type) {
        Some(MotionKind::Accelerometer) => {
            apply_accel_milli(
                &mut state.device,
                motion.x_milli,
                motion.y_milli,
                motion.z_milli,
            );
        }
        Some(MotionKind::Gyroscope) => {
            apply_gyro_milli(
                &mut state.device,
                motion.x_milli,
                motion.y_milli,
                motion.z_milli,
            );
        }
        None => return false,
    }
    state.last_motion_us = if now_us != 0 { now_us } else { 1 };
    true
}

pub fn apply_battery(battery: &BatteryStateRequest, state: &mut Sc26State) -> bool {
    let st = &mut state.device;
    if battery.percent <= 100 {
        st.battery_percent = battery.percent;
    }
    match BatteryState::from_raw(battery.flags) {
        BatteryState::Charging => st.charge_state = CHARGE_CHARGING,
        BatteryState::Full => {
            st.charge_state = CHARGE_DONE;
            st.battery_percent = 100;
        }
        _ => st.charge_state = CHARGE_DISCHARGING,
    }
    true
}
