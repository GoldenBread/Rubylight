//! Controller arrival, capabilities, device classification, back-as-guide,
//! handheld control detection, launch string formatting, and Steam Controller
//! (2026) on-screen controls merging matching `ControllerHandler` and `SteamControls`.

use rubylight_protocol::steam_controller as sc;

use crate::controller::controller_type;
use crate::controller::deadzone::QUIRK_TRIGGER_DEADZONE;
use crate::controller::mapping::{keycode, quirk};
use crate::gamepad_constants::{button, capability};

/// Android `MotionEvent` axis IDs used by `classify_input_device`.
pub mod axis {
    pub const Z: i32 = 11;
    pub const RX: i32 = 12;
    pub const RY: i32 = 13;
    pub const RZ: i32 = 14;
    pub const LTRIGGER: i32 = 17;
    pub const RTRIGGER: i32 = 18;
    pub const THROTTLE: i32 = 19;
    pub const GAS: i32 = 22;
    pub const BRAKE: i32 = 23;
}

/// Buttons announced by the on-screen controller.
pub const ONSCREEN_PAD_BUTTONS: u32 = button::A
    | button::B
    | button::X
    | button::Y
    | button::UP
    | button::DOWN
    | button::LEFT
    | button::RIGHT
    | button::LB
    | button::RB
    | button::LS_CLK
    | button::RS_CLK
    | button::BACK
    | button::PLAY
    | button::SPECIAL
    | button::TOUCHPAD;

/// Back buttons added by `SteamControls` (`R4, L4, R5, L5 = PADDLE1..4`).
pub const STEAM_CONTROLS_BUTTONS: u32 =
    button::PADDLE1 | button::PADDLE2 | button::PADDLE3 | button::PADDLE4;

/// Arrival capabilities added by `SteamControls` (`GRIP_SENSE | STICK_TOUCH | SC26_BUTTONS`).
pub const STEAM_CONTROLS_CAPABILITIES: u16 =
    sc::LI_CCAP_GRIP_SENSE | sc::LI_CCAP_STICK_TOUCH | sc::LI_CCAP_SC26_BUTTONS;

/// Trigger pull threshold (`0xF0`) matching `SteamControls.TRIGGER_CLICK_THRESHOLD`.
pub const TRIGGER_CLICK_THRESHOLD: u8 = 0xF0;

pub fn counts_as_external_controller(
    external: bool,
    gamepad: bool,
    remote: bool,
    keyboard: bool,
    handheld_controls: bool,
) -> bool {
    external && gamepad && !remote && !keyboard && !handheld_controls
}

pub fn onscreen_owns_player_one(
    onscreen_own_player: bool,
    multi_controller: bool,
    onscreen_controller: bool,
    only_l3_r3: bool,
) -> bool {
    onscreen_own_player && multi_controller && onscreen_controller && !only_l3_r3
}

pub fn looks_like_handheld_controls(
    name: Option<&str>,
    vendor_id: i32,
    only_gamepad: bool,
    attached_usb: bool,
) -> bool {
    !attached_usb
        && only_gamepad
        && name.is_some_and(|n| n.to_lowercase().contains("xbox"))
        && vendor_id != 0x045e
        && vendor_id != 0
}

pub fn is_back_as_guide(back_as_guide: bool, key_code: i32, flags: i32) -> bool {
    back_as_guide && key_code == keycode::BACK && (flags & keycode::FLAG_SOFT_KEYBOARD) == 0
}

pub fn is_pad_back_as_guide(
    back_as_guide: bool,
    key_code: i32,
    flags: i32,
    back_is_start: bool,
) -> bool {
    is_back_as_guide(back_as_guide, key_code, flags) && !back_is_start
}

#[allow(clippy::too_many_arguments)]
pub fn is_handheld_system_back(
    key_code: i32,
    flags: i32,
    source: i32,
    virtual_device: bool,
    external: bool,
    alphabetic_keyboard: bool,
    gamepad: bool,
    back_is_spare: bool,
) -> bool {
    key_code == keycode::BACK
        && back_is_spare
        && (flags
            & (keycode::FLAG_VIRTUAL_HARD_KEY
                | keycode::FLAG_FALLBACK
                | keycode::FLAG_SOFT_KEYBOARD))
            == 0
        && (source & keycode::SOURCE_CLASS_POINTER) == 0
        && (virtual_device || (!external && !alphabetic_keyboard && !gamepad))
}

pub fn back_is_spare_button(
    internal_gamepad: bool,
    internal_select: bool,
    any_gamepad: bool,
) -> bool {
    any_gamepad && (!internal_gamepad || internal_select)
}

/// Returns `Some(true/false)` for hardcoded device external overrides, or `None` to query Android.
pub fn is_external_override(build_model: &str, device_name: &str) -> Option<bool> {
    if build_model == "Tinker Board" {
        return Some(true);
    }
    if device_name.contains("gpio")
        || device_name.contains("joy_key")
        || device_name.contains("keypad")
        || device_name.eq_ignore_ascii_case("NVIDIA Corporation NVIDIA Controller v01.01")
        || device_name.eq_ignore_ascii_case("NVIDIA Corporation NVIDIA Controller v01.02")
        || device_name.eq_ignore_ascii_case("GR0006")
    {
        return Some(false);
    }
    None
}

pub fn should_ignore_back(
    dev_name: &str,
    has_joystick_axes: bool,
    has_gamepad_buttons: bool,
    external: bool,
    internal_gamepad: bool,
    internal_select: bool,
) -> bool {
    if dev_name.contains("Razer Serval") {
        return true;
    }
    if !has_joystick_axes && dev_name.to_lowercase().contains("remote") {
        return true;
    }
    if !external {
        !internal_gamepad || internal_select
    } else {
        !has_joystick_axes && !has_gamepad_buttons
    }
}

pub fn arrival_type(detected: u8, connect_as: u8, target_button_flags: u32) -> u8 {
    if connect_as != controller_type::UNKNOWN {
        return connect_as;
    }
    if (target_button_flags & button::TOUCHPAD) != 0 {
        controller_type::PS
    } else {
        detected
    }
}

pub fn drops_motion_and_touchpad(connect_as: u8) -> bool {
    connect_as == controller_type::XBOX
}

pub fn arrival_capabilities(capabilities: u16, connect_as: u8) -> u16 {
    if !drops_motion_and_touchpad(connect_as) {
        capabilities
    } else {
        capabilities
            & !(capability::ACCEL
                | capability::GYRO
                | capability::TOUCHPAD
                | capability::DUAL_TOUCHPAD)
    }
}

pub fn motion_overrides_type(detected: u8, has_motion: bool, connect_as: u8) -> bool {
    detected != controller_type::PS && has_motion && connect_as == controller_type::UNKNOWN
}

pub fn input_device_arrival_type(
    detected: u8,
    has_motion: bool,
    connect_as: u8,
    target_button_flags: u32,
) -> u8 {
    let base = if motion_overrides_type(detected, has_motion, connect_as) {
        controller_type::UNKNOWN
    } else {
        detected
    };
    arrival_type(base, connect_as, target_button_flags)
}

pub fn takes_device_motion(
    fallback_to_device: bool,
    relative_slot: i16,
    pad_capabilities: u16,
    connect_as: u8,
) -> bool {
    fallback_to_device
        && relative_slot == 0
        && (pad_capabilities & (capability::ACCEL | capability::GYRO)) == 0
        && !drops_motion_and_touchpad(connect_as)
}

pub fn needs_clickpad_emulation(target_button_flags: u32) -> bool {
    (target_button_flags & button::TOUCHPAD) == 0
}

pub fn onscreen_pad_type(touchpad_button: bool) -> u8 {
    if touchpad_button {
        controller_type::PS
    } else {
        controller_type::UNKNOWN
    }
}

pub fn onscreen_pad_buttons(touchpad_button: bool, steam_controls: bool) -> u32 {
    let buttons = if touchpad_button {
        ONSCREEN_PAD_BUTTONS
    } else {
        ONSCREEN_PAD_BUTTONS & !button::TOUCHPAD
    };
    if steam_controls {
        buttons | STEAM_CONTROLS_BUTTONS
    } else {
        buttons
    }
}

pub fn onscreen_pad_capabilities(steam_controls: bool) -> u16 {
    if steam_controls {
        STEAM_CONTROLS_CAPABILITIES
    } else {
        0
    }
}

pub fn carries_steam_controls(
    steam_controls: bool,
    onscreen_own_player: bool,
    controller_number: i16,
) -> bool {
    steam_controls && !onscreen_own_player && controller_number == 0
}

pub fn with_steam_controls_capabilities(
    steam_controls: bool,
    onscreen_own_player: bool,
    controller_number: i16,
    capabilities: u16,
    connect_as: u8,
) -> u16 {
    if !carries_steam_controls(steam_controls, onscreen_own_player, controller_number) {
        return capabilities;
    }
    arrival_capabilities(capabilities | STEAM_CONTROLS_CAPABILITIES, connect_as)
}

pub fn with_steam_controls_buttons(
    steam_controls: bool,
    onscreen_own_player: bool,
    controller_number: i16,
    buttons: u32,
) -> u32 {
    if carries_steam_controls(steam_controls, onscreen_own_player, controller_number) {
        buttons | STEAM_CONTROLS_BUTTONS
    } else {
        buttons
    }
}

/// Touchpad click flag for `last_touch_x`: left half clicks left pad; right half or `NaN` clicks
/// right pad.
pub fn steam_touchpad_click_flag(last_touch_x: f32) -> u32 {
    if last_touch_x < 0.5 {
        sc::LEFT_PAD_CLICK_FLAG
    } else {
        sc::RIGHT_PAD_CLICK_FLAG
    }
}

pub fn steam_with_touchpad_click(buttons: u32, last_touch_x: f32) -> u32 {
    if (buttons & button::TOUCHPAD) == 0 {
        buttons
    } else {
        buttons | steam_touchpad_click_flag(last_touch_x)
    }
}

/// Merges SC26 trigger end-stop clicks and SteamControls touchpad click on slot 0.
pub fn apply_steam_controls_input(
    steam_controls_session: bool,
    controller_number: i16,
    mut input_map: u32,
    left_trigger: u8,
    right_trigger: u8,
    steam_pad_touch_x: f32,
) -> u32 {
    if steam_controls_session && controller_number == 0 {
        if left_trigger >= TRIGGER_CLICK_THRESHOLD {
            input_map |= sc::LEFT_TRIGGER_CLICK_FLAG;
        }
        if right_trigger >= TRIGGER_CLICK_THRESHOLD {
            input_map |= sc::RIGHT_TRIGGER_CLICK_FLAG;
        }
        input_map = steam_with_touchpad_click(input_map, steam_pad_touch_x);
    }
    input_map
}

pub fn launch_controller(slot: u8, kind: u8, capabilities: u16, buttons: u32) -> String {
    let kind = if kind == controller_type::UNKNOWN {
        controller_type::XBOX
    } else {
        kind
    };
    format!("{slot}:{kind}:{capabilities}:{buttons}")
}

/// Output of `classify_input_device` matching `createInputDeviceContextForDevice`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassifiedInputDevice {
    pub right_stick_x_axis: i32,
    pub right_stick_y_axis: i32,
    pub left_trigger_axis: i32,
    pub right_trigger_axis: i32,
    pub triggers_idle_negative: bool,
    pub quirk_flags: u32,
    pub has_select: bool,
    pub has_mode: bool,
    pub trigger_deadzone: f32,
}

#[allow(clippy::too_many_arguments)]
pub fn classify_input_device(
    vendor_id: i32,
    product_id: i32,
    dev_name: Option<&str>,
    is_keyboard_mouse_only: bool,
    has_ltrigger_rtrigger: bool,
    has_brake_gas: bool,
    has_brake_throttle: bool,
    has_gas_range: bool,
    has_rx_ry: bool,
    has_z_rz: bool,
    has_button_c: bool,
    has_hat: bool,
    has_start_or_menu: bool,
    initial_has_select: bool,
    initial_has_mode: bool,
    ignore_back: bool,
    base_trigger_deadzone: f32,
) -> ClassifiedInputDevice {
    let mut rs_x = -1;
    let mut rs_y = -1;
    let mut lt = -1;
    let mut rt = -1;
    let mut triggers_idle_negative = false;
    let mut has_select = initial_has_select;
    let mut has_mode = initial_has_mode;
    let mut trigger_deadzone = base_trigger_deadzone;
    let mut quirks = if ignore_back { quirk::IGNORE_BACK } else { 0 };
    if !has_hat {
        quirks |= quirk::NO_HAT;
    }
    if vendor_id == 0x054c
        && dev_name.is_some_and(|n| n.ends_with(" Touchpad"))
        && is_keyboard_mouse_only
    {
        quirks |= quirk::DS4_STANDALONE_TOUCHPAD;
    }

    if has_ltrigger_rtrigger {
        lt = axis::LTRIGGER;
        rt = axis::RTRIGGER;
    } else if has_brake_gas {
        lt = axis::BRAKE;
        rt = axis::GAS;
    } else if has_brake_throttle {
        lt = axis::BRAKE;
        rt = axis::THROTTLE;
    } else if has_rx_ry && dev_name.is_some() {
        let mut non_std_ds4 = false;
        if vendor_id == 0x054c {
            if has_button_c {
                non_std_ds4 = true;
                quirks |= quirk::NON_STD_DS4;
            } else {
                quirks |= quirk::LINUX_STD_FACE;
            }
        }
        if non_std_ds4 {
            lt = axis::RX;
            rt = axis::RY;
            has_select = true;
            has_mode = true;
        } else {
            rs_x = axis::RX;
            rs_y = axis::RY;
            if has_z_rz {
                lt = axis::Z;
                rt = axis::RZ;
            }
        }
        triggers_idle_negative = true;
    }

    if rs_x == -1 && rs_y == -1 {
        if has_z_rz {
            rs_x = axis::Z;
            rs_y = axis::RZ;
        } else if has_rx_ry {
            rs_x = axis::RX;
            rs_y = axis::RY;
        }
    }

    if vendor_id == 0x18d1 && product_id == 0x2c40 {
        quirks |= quirk::BACK_IS_START | quirk::MODE_IS_SELECT;
        trigger_deadzone = trigger_deadzone.max(QUIRK_TRIGGER_DEADZONE);
        has_select = true;
        has_mode = false;
    }

    if let Some(name) = dev_name {
        if name.contains("ASUS Gamepad") {
            if !has_start_or_menu {
                quirks |= quirk::BACK_IS_START | quirk::MODE_IS_SELECT;
                has_select = true;
                has_mode = false;
            }
            trigger_deadzone = trigger_deadzone.max(QUIRK_TRIGGER_DEADZONE);
        } else if name.contains("SHIELD") || name.contains("NVIDIA Controller") {
            if name.contains("NVIDIA Controller v01.03")
                || name.contains("NVIDIA Controller v01.04")
            {
                quirks |= quirk::SEARCH_IS_MODE;
                has_mode = true;
            }
        } else if name.contains("Razer Serval") {
            quirks |= quirk::SERVAL;
            has_mode = true;
            has_select = true;
        } else if name == "Xbox Wireless Controller" && !has_gas_range {
            quirks |= quirk::NON_STD_XBOX_BT;
            has_mode = true;
            has_select = true;
        }
    }

    if vendor_id == 0x044f && product_id == 0xb328 {
        has_mode = false;
    }

    ClassifiedInputDevice {
        right_stick_x_axis: rs_x,
        right_stick_y_axis: rs_y,
        left_trigger_axis: lt,
        right_trigger_axis: rt,
        triggers_idle_negative,
        quirk_flags: quirks,
        has_select,
        has_mode,
        trigger_deadzone,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputDeviceArrival {
    pub reported_type: u8,
    pub reported_capabilities: u16,
    pub supported_buttons: u32,
    pub motion_overrides: bool,
    pub set_needs_clickpad: bool,
    pub needs_clickpad_value: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn compute_input_device_arrival(
    detected_type: u8,
    base_supported_buttons: u32,
    base_capabilities: u16,
    has_motion: bool,
    connect_as: u8,
    target_button_flags: u32,
    carries_touchpad_button: bool,
    steam_controls_session: bool,
    onscreen_own_player_session: bool,
    controller_number: i16,
) -> InputDeviceArrival {
    let motion_overrides = motion_overrides_type(detected_type, has_motion, connect_as);
    let mut set_needs_clickpad = false;
    let mut needs_clickpad_value = false;
    if motion_overrides
        || (connect_as == controller_type::PS && detected_type != controller_type::PS)
    {
        set_needs_clickpad = true;
        needs_clickpad_value = needs_clickpad_emulation(target_button_flags);
    }
    let mut reported_type =
        input_device_arrival_type(detected_type, has_motion, connect_as, target_button_flags);
    let mut supported_buttons = base_supported_buttons;
    if carries_touchpad_button {
        reported_type = controller_type::PS;
        supported_buttons |= button::TOUCHPAD;
    }
    let caps = arrival_capabilities(base_capabilities, connect_as);
    let reported_capabilities = with_steam_controls_capabilities(
        steam_controls_session,
        onscreen_own_player_session,
        controller_number,
        caps,
        connect_as,
    );
    let supported_buttons = with_steam_controls_buttons(
        steam_controls_session,
        onscreen_own_player_session,
        controller_number,
        supported_buttons,
    );
    InputDeviceArrival {
        reported_type,
        reported_capabilities,
        supported_buttons,
        motion_overrides,
        set_needs_clickpad,
        needs_clickpad_value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handheld_back_and_controls_checks_match_java() {
        assert!(back_is_spare_button(false, false, true));
        assert!(back_is_spare_button(true, true, true));
        assert!(!back_is_spare_button(true, false, true));
        assert!(!back_is_spare_button(false, false, false));

        assert!(is_handheld_system_back(
            keycode::BACK,
            0,
            0x101,
            true,
            false,
            true,
            false,
            true
        ));
        assert!(!is_handheld_system_back(
            keycode::BACK,
            keycode::FLAG_VIRTUAL_HARD_KEY,
            0x101,
            true,
            false,
            true,
            false,
            true
        ));
        assert!(!is_handheld_system_back(
            keycode::BACK,
            0,
            keycode::SOURCE_CLASS_POINTER,
            true,
            false,
            true,
            false,
            true
        ));

        assert!(looks_like_handheld_controls(
            Some("Xbox Wireless Controller"),
            0x2022,
            true,
            false
        ));
        assert!(!looks_like_handheld_controls(
            Some("Xbox Wireless Controller"),
            0x045e,
            true,
            false
        ));
        assert!(!looks_like_handheld_controls(
            Some("Xbox Wireless Controller"),
            0x2022,
            true,
            true
        ));
    }

    #[test]
    fn steam_controls_and_capabilities_respect_pins_and_nan() {
        assert_eq!(
            onscreen_pad_buttons(false, false) & (button::TOUCHPAD | STEAM_CONTROLS_BUTTONS),
            0
        );
        assert_eq!(
            onscreen_pad_buttons(true, true) & (button::TOUCHPAD | STEAM_CONTROLS_BUTTONS),
            button::TOUCHPAD | STEAM_CONTROLS_BUTTONS
        );
        assert_eq!(onscreen_pad_capabilities(true), STEAM_CONTROLS_CAPABILITIES);
        assert!(carries_steam_controls(true, false, 0));
        assert!(!carries_steam_controls(true, true, 0));
        assert!(!carries_steam_controls(true, false, 1));

        // Pinned to Xbox drops touchpad & motion, keeps non-touchpad Steam caps.
        let caps = arrival_capabilities(
            capability::GYRO | capability::TOUCHPAD | capability::RUMBLE,
            controller_type::XBOX,
        );
        assert_eq!(caps, capability::RUMBLE);

        // NaN touchX defaults to RIGHT_PAD_CLICK_FLAG; < 0.5 chooses LEFT_PAD_CLICK_FLAG.
        assert_eq!(
            apply_steam_controls_input(true, 0, button::TOUCHPAD, 0xF0, 0xEF, f32::NAN),
            button::TOUCHPAD | sc::LEFT_TRIGGER_CLICK_FLAG | sc::RIGHT_PAD_CLICK_FLAG
        );
        assert_eq!(
            apply_steam_controls_input(true, 0, button::TOUCHPAD, 0, 0xFF, 0.25),
            button::TOUCHPAD | sc::RIGHT_TRIGGER_CLICK_FLAG | sc::LEFT_PAD_CLICK_FLAG
        );
        assert_eq!(
            launch_controller(0, controller_type::UNKNOWN, 0x41, 0xFFFF),
            "0:1:65:65535"
        );
    }

    #[test]
    fn classify_input_device_handles_ds4_and_adt1() {
        let ds4 = classify_input_device(
            0x054c,
            0x05c4,
            Some("Wireless Controller"),
            false,
            false,
            false,
            false,
            false,
            true,
            true,
            true,
            true,
            true,
            false,
            false,
            false,
            0.02,
        );
        assert_ne!(ds4.quirk_flags & quirk::NON_STD_DS4, 0);
        assert_eq!(ds4.left_trigger_axis, axis::RX);
        assert_eq!(ds4.right_trigger_axis, axis::RY);
        assert!(ds4.triggers_idle_negative);

        let adt1 = classify_input_device(
            0x18d1,
            0x2c40,
            Some("ADT-1"),
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            false,
            false,
            false,
            false,
            0.05,
        );
        assert_ne!(adt1.quirk_flags & quirk::BACK_IS_START, 0);
        assert_ne!(adt1.quirk_flags & quirk::MODE_IS_SELECT, 0);
        assert!((adt1.trigger_deadzone - QUIRK_TRIGGER_DEADZONE).abs() < 1e-6);
        assert!(adt1.has_select && !adt1.has_mode);
    }
}
