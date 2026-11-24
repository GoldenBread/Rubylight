//! Emulated controller button combos (`EMULATING_SELECT`, `EMULATING_SPECIAL`,
//! `EMULATING_TOUCHPAD`) and USB Select+LB clickpad emulation matching
//! `ControllerHandler` and `UsbDeviceContext.clickpadButtons`.

use crate::controller::controller_type;
use crate::controller::mapping::{
    EMULATING_SELECT, EMULATING_SPECIAL, EMULATING_TOUCHPAD, MAXIMUM_BUMPER_UP_DELAY_MS,
};
use crate::gamepad_constants::button;

/// Returns the physically held button mask, restoring Select/LB swapped out by clickpad emulation.
pub fn held_buttons(
    input_map: u32,
    emulating_flags: u32,
    has_target_touchpad: bool,
    has_select: bool,
    has_lb: bool,
) -> u32 {
    let mut held = input_map;
    if (emulating_flags & EMULATING_TOUCHPAD) != 0 {
        if !has_target_touchpad {
            held &= !button::TOUCHPAD;
        }
        if has_select {
            held |= button::BACK;
        }
        if has_lb {
            held |= button::LB;
        }
    }
    held
}

/// Applies emulated Select / Touchpad / Special combos on key-down.
#[allow(clippy::too_many_arguments)]
pub fn apply_emulated_combos_on_down(
    mut input_map: u32,
    mut emulating_flags: u32,
    has_select: bool,
    has_mode: bool,
    needs_clickpad: bool,
    event_time_ms: i64,
    last_lb_up_ms: i64,
    last_rb_up_ms: i64,
) -> (u32, u32) {
    if !has_select {
        if input_map == (button::PLAY | button::LB)
            || (input_map == button::PLAY
                && event_time_ms.wrapping_sub(last_lb_up_ms) <= MAXIMUM_BUMPER_UP_DELAY_MS)
        {
            input_map &= !(button::PLAY | button::LB);
            input_map |= button::BACK;
            emulating_flags |= EMULATING_SELECT;
        }
    } else if needs_clickpad
        && (input_map == (button::BACK | button::LB)
            || (input_map == button::BACK
                && event_time_ms.wrapping_sub(last_lb_up_ms) <= MAXIMUM_BUMPER_UP_DELAY_MS))
    {
        input_map &= !(button::BACK | button::LB);
        input_map |= button::TOUCHPAD;
        emulating_flags |= EMULATING_TOUCHPAD;
    }

    if !has_mode {
        if has_select {
            if input_map == (button::PLAY | button::BACK) {
                input_map &= !(button::PLAY | button::BACK);
                input_map |= button::SPECIAL;
                emulating_flags |= EMULATING_SPECIAL;
            }
        } else if input_map == (button::PLAY | button::RB)
            || (input_map == button::PLAY
                && event_time_ms.wrapping_sub(last_rb_up_ms) <= MAXIMUM_BUMPER_UP_DELAY_MS)
        {
            input_map &= !(button::PLAY | button::RB);
            input_map |= button::SPECIAL;
            emulating_flags |= EMULATING_SPECIAL;
        }
    }
    (input_map, emulating_flags)
}

/// Releases emulated Select / Special / Touchpad combos when prerequisite buttons lift.
pub fn apply_emulated_combos_on_up(
    mut input_map: u32,
    mut emulating_flags: u32,
    has_target_touchpad: bool,
    has_select: bool,
    has_lb: bool,
) -> (u32, u32) {
    if (emulating_flags & EMULATING_SELECT) != 0
        && ((input_map & button::PLAY) == 0 || (input_map & button::LB) == 0)
    {
        input_map &= !button::BACK;
        emulating_flags &= !EMULATING_SELECT;
    }
    if (emulating_flags & EMULATING_SPECIAL) != 0
        && ((input_map & button::PLAY) == 0
            || ((input_map & button::BACK) == 0 && (input_map & button::RB) == 0))
    {
        input_map &= !button::SPECIAL;
        emulating_flags &= !EMULATING_SPECIAL;
    }
    if (emulating_flags & EMULATING_TOUCHPAD) != 0 {
        let held = held_buttons(
            input_map,
            emulating_flags,
            has_target_touchpad,
            has_select,
            has_lb,
        );
        if (held & button::BACK) == 0 || (held & button::LB) == 0 {
            input_map &= !button::TOUCHPAD;
            emulating_flags &= !EMULATING_TOUCHPAD;
        }
    }
    (input_map, emulating_flags)
}

/// USB driver Select+LB clickpad emulation matching `UsbDeviceContext.clickpadButtons`.
pub fn apply_usb_clickpad(
    buttons: u32,
    mut clickpad_swallowed: u32,
    device_type: u8,
    connect_as: u8,
    has_device_motion: bool,
    needs_clickpad: bool,
) -> (u32, u32) {
    let faked_as_ps = connect_as == controller_type::PS
        || crate::controller::arrival::motion_overrides_type(
            device_type,
            has_device_motion,
            connect_as,
        );
    if !faked_as_ps || device_type == controller_type::PS || !needs_clickpad {
        return (buttons, clickpad_swallowed);
    }
    let combo = button::BACK | button::LB;
    if clickpad_swallowed == 0 && buttons == combo {
        clickpad_swallowed = combo;
    }
    clickpad_swallowed &= buttons;
    let out = (buttons & !clickpad_swallowed)
        | if clickpad_swallowed == combo {
            button::TOUCHPAD
        } else {
            0
        };
    (out, clickpad_swallowed)
}
