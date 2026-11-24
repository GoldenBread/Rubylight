//! JNI exports for `nightjar_policy::controller::arrival`.

use jni::JNIEnv;
use jni::objects::{JBooleanArray, JClass, JFloatArray, JIntArray, JString};
use jni::sys::{JNI_FALSE, jboolean, jbyte, jfloat, jint, jshort, jstring};
use nightjar_policy::controller::arrival::{
    arrival_capabilities, arrival_type, back_is_spare_button, carries_steam_controls,
    classify_input_device, compute_input_device_arrival, counts_as_external_controller,
    drops_motion_and_touchpad, input_device_arrival_type, is_back_as_guide, is_external_override,
    is_handheld_system_back, is_pad_back_as_guide, launch_controller, looks_like_handheld_controls,
    motion_overrides_type, needs_clickpad_emulation, onscreen_owns_player_one,
    onscreen_pad_buttons, onscreen_pad_capabilities, onscreen_pad_type, should_ignore_back,
    takes_device_motion, with_steam_controls_buttons, with_steam_controls_capabilities,
};
use nightjar_policy::controller::mapping::{ANDROID_TO_LI_BUTTON_MAP, supported_buttons_from_keys};
use nightjar_policy::gamepad_constants::button;

use super::{new_jstring, write_floats, write_ints};
use crate::policy::jbool;
use crate::string;

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_countsAsExternalController(
    _: JNIEnv,
    _: JClass,
    external: jboolean,
    gamepad: jboolean,
    remote: jboolean,
    keyboard: jboolean,
    handheld_controls: jboolean,
) -> jboolean {
    jbool(counts_as_external_controller(
        external != JNI_FALSE,
        gamepad != JNI_FALSE,
        remote != JNI_FALSE,
        keyboard != JNI_FALSE,
        handheld_controls != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_onscreenOwnsPlayerOne(
    _: JNIEnv,
    _: JClass,
    onscreen_own_player: jboolean,
    multi_controller: jboolean,
    onscreen_controller: jboolean,
    only_l3_r3: jboolean,
) -> jboolean {
    jbool(onscreen_owns_player_one(
        onscreen_own_player != JNI_FALSE,
        multi_controller != JNI_FALSE,
        onscreen_controller != JNI_FALSE,
        only_l3_r3 != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_looksLikeHandheldControls(
    mut env: JNIEnv,
    _: JClass,
    name: JString,
    vendor_id: jint,
    only_gamepad: jboolean,
    attached_usb: jboolean,
) -> jboolean {
    let name = string(&mut env, &name);
    jbool(looks_like_handheld_controls(
        name.as_deref(),
        vendor_id,
        only_gamepad != JNI_FALSE,
        attached_usb != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_isBackAsGuide(
    _: JNIEnv,
    _: JClass,
    back_as_guide: jboolean,
    key_code: jint,
    flags: jint,
) -> jboolean {
    jbool(is_back_as_guide(
        back_as_guide != JNI_FALSE,
        key_code,
        flags,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_isPadBackAsGuide(
    _: JNIEnv,
    _: JClass,
    back_as_guide: jboolean,
    key_code: jint,
    flags: jint,
    back_is_start: jboolean,
) -> jboolean {
    jbool(is_pad_back_as_guide(
        back_as_guide != JNI_FALSE,
        key_code,
        flags,
        back_is_start != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_isHandheldSystemBack(
    _: JNIEnv,
    _: JClass,
    key_code: jint,
    flags: jint,
    source: jint,
    virtual_device: jboolean,
    external: jboolean,
    alphabetic_keyboard: jboolean,
    gamepad: jboolean,
    back_is_spare_button: jboolean,
) -> jboolean {
    jbool(is_handheld_system_back(
        key_code,
        flags,
        source,
        virtual_device != JNI_FALSE,
        external != JNI_FALSE,
        alphabetic_keyboard != JNI_FALSE,
        gamepad != JNI_FALSE,
        back_is_spare_button != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_backIsSpareButton(
    _: JNIEnv,
    _: JClass,
    internal_gamepad: jboolean,
    internal_select: jboolean,
    any_gamepad: jboolean,
) -> jboolean {
    jbool(back_is_spare_button(
        internal_gamepad != JNI_FALSE,
        internal_select != JNI_FALSE,
        any_gamepad != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_isExternalOverride(
    mut env: JNIEnv,
    _: JClass,
    build_model: JString,
    dev_name: JString,
) -> jint {
    let model = string(&mut env, &build_model).unwrap_or_default();
    let name = string(&mut env, &dev_name).unwrap_or_default();
    match is_external_override(&model, &name) {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_shouldIgnoreBack(
    mut env: JNIEnv,
    _: JClass,
    dev_name: JString,
    has_joystick_axes: jboolean,
    has_gamepad_buttons: jboolean,
    external: jboolean,
    internal_gamepad: jboolean,
    internal_select: jboolean,
) -> jboolean {
    let name = string(&mut env, &dev_name).unwrap_or_default();
    jbool(should_ignore_back(
        &name,
        has_joystick_axes != JNI_FALSE,
        has_gamepad_buttons != JNI_FALSE,
        external != JNI_FALSE,
        internal_gamepad != JNI_FALSE,
        internal_select != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_arrivalType(
    _: JNIEnv,
    _: JClass,
    detected_type: jbyte,
    connect_as: jbyte,
    target_button_flags: jint,
) -> jbyte {
    arrival_type(
        detected_type as u8,
        connect_as as u8,
        target_button_flags as u32,
    ) as jbyte
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_inputDeviceArrivalType(
    _: JNIEnv,
    _: JClass,
    detected_type: jbyte,
    has_motion: jboolean,
    connect_as: jbyte,
    target_button_flags: jint,
) -> jbyte {
    input_device_arrival_type(
        detected_type as u8,
        has_motion != JNI_FALSE,
        connect_as as u8,
        target_button_flags as u32,
    ) as jbyte
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_dropsMotionAndTouchpad(
    _: JNIEnv,
    _: JClass,
    connect_as: jbyte,
) -> jboolean {
    jbool(drops_motion_and_touchpad(connect_as as u8))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_arrivalCapabilities(
    _: JNIEnv,
    _: JClass,
    capabilities: jshort,
    connect_as: jbyte,
) -> jshort {
    arrival_capabilities(capabilities as u16, connect_as as u8) as jshort
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_takesDeviceMotion(
    _: JNIEnv,
    _: JClass,
    fallback_to_device: jboolean,
    relative_slot: jshort,
    pad_capabilities: jshort,
    connect_as: jbyte,
) -> jboolean {
    jbool(takes_device_motion(
        fallback_to_device != JNI_FALSE,
        relative_slot,
        pad_capabilities as u16,
        connect_as as u8,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_motionOverridesType(
    _: JNIEnv,
    _: JClass,
    detected_type: jbyte,
    has_motion: jboolean,
    connect_as: jbyte,
) -> jboolean {
    jbool(motion_overrides_type(
        detected_type as u8,
        has_motion != JNI_FALSE,
        connect_as as u8,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_needsClickpadEmulation(
    _: JNIEnv,
    _: JClass,
    target_button_flags: jint,
) -> jboolean {
    jbool(needs_clickpad_emulation(target_button_flags as u32))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_onscreenPadType(
    _: JNIEnv,
    _: JClass,
    touchpad_button: jboolean,
) -> jbyte {
    onscreen_pad_type(touchpad_button != JNI_FALSE) as jbyte
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_onscreenPadButtons(
    _: JNIEnv,
    _: JClass,
    touchpad_button: jboolean,
    steam_controls: jboolean,
) -> jint {
    onscreen_pad_buttons(touchpad_button != JNI_FALSE, steam_controls != JNI_FALSE) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_onscreenPadCapabilities(
    _: JNIEnv,
    _: JClass,
    steam_controls: jboolean,
) -> jshort {
    onscreen_pad_capabilities(steam_controls != JNI_FALSE) as jshort
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_carriesSteamControls(
    _: JNIEnv,
    _: JClass,
    steam_controls: jboolean,
    onscreen_own_player: jboolean,
    controller_number: jshort,
) -> jboolean {
    jbool(carries_steam_controls(
        steam_controls != JNI_FALSE,
        onscreen_own_player != JNI_FALSE,
        controller_number,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_withSteamControlsCapabilities(
    _: JNIEnv,
    _: JClass,
    steam_controls: jboolean,
    onscreen_own_player: jboolean,
    controller_number: jshort,
    capabilities: jshort,
    connect_as: jbyte,
) -> jshort {
    with_steam_controls_capabilities(
        steam_controls != JNI_FALSE,
        onscreen_own_player != JNI_FALSE,
        controller_number,
        capabilities as u16,
        connect_as as u8,
    ) as jshort
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_withSteamControlsButtons(
    _: JNIEnv,
    _: JClass,
    steam_controls: jboolean,
    onscreen_own_player: jboolean,
    controller_number: jshort,
    buttons: jint,
) -> jint {
    with_steam_controls_buttons(
        steam_controls != JNI_FALSE,
        onscreen_own_player != JNI_FALSE,
        controller_number,
        buttons as u32,
    ) as jint
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_classifyInputDevice(
    mut env: JNIEnv,
    _: JClass,
    vendor_id: jint,
    product_id: jint,
    dev_name: JString,
    is_keyboard_mouse_only: jboolean,
    has_ltrigger_rtrigger: jboolean,
    has_brake_gas: jboolean,
    has_brake_throttle: jboolean,
    has_gas_range: jboolean,
    has_rx_ry: jboolean,
    has_z_rz: jboolean,
    has_button_c: jboolean,
    has_hat: jboolean,
    has_start_or_menu: jboolean,
    initial_has_select: jboolean,
    initial_has_mode: jboolean,
    ignore_back: jboolean,
    base_trigger_deadzone: jfloat,
    out_ints: JIntArray,
    out_trigger_deadzone: JFloatArray,
) {
    let name = string(&mut env, &dev_name);
    let res = classify_input_device(
        vendor_id,
        product_id,
        name.as_deref(),
        is_keyboard_mouse_only != JNI_FALSE,
        has_ltrigger_rtrigger != JNI_FALSE,
        has_brake_gas != JNI_FALSE,
        has_brake_throttle != JNI_FALSE,
        has_gas_range != JNI_FALSE,
        has_rx_ry != JNI_FALSE,
        has_z_rz != JNI_FALSE,
        has_button_c != JNI_FALSE,
        has_hat != JNI_FALSE,
        has_start_or_menu != JNI_FALSE,
        initial_has_select != JNI_FALSE,
        initial_has_mode != JNI_FALSE,
        ignore_back != JNI_FALSE,
        base_trigger_deadzone,
    );
    let bool_bits = i32::from(res.triggers_idle_negative)
        | (i32::from(res.has_select) << 1)
        | (i32::from(res.has_mode) << 2);
    write_ints(
        &mut env,
        &out_ints,
        &[
            res.right_stick_x_axis,
            res.right_stick_y_axis,
            res.left_trigger_axis,
            res.right_trigger_axis,
            res.quirk_flags as i32,
            bool_bits,
        ],
    );
    write_floats(&mut env, &out_trigger_deadzone, &[res.trigger_deadzone]);
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_computeBaseSupportedButtons(
    env: JNIEnv,
    _: JClass,
    has_keys: JBooleanArray,
    has_paddles: jboolean,
    has_share: jboolean,
    target_button_flags: jint,
    has_hat_x: jboolean,
    has_hat_y: jboolean,
    has_clickpad: jboolean,
) -> jint {
    let mut keys_buf = [JNI_FALSE; ANDROID_TO_LI_BUTTON_MAP.len()];
    if !has_keys.is_null() {
        let len = env.get_array_length(&has_keys).unwrap_or(0).max(0) as usize;
        let read_len = len.min(keys_buf.len());
        if read_len > 0 {
            let _ = env.get_boolean_array_region(&has_keys, 0, &mut keys_buf[..read_len]);
        }
    }
    let bools: Vec<bool> = keys_buf.iter().map(|&v| v != JNI_FALSE).collect();
    let mut flags = supported_buttons_from_keys(&bools);
    if has_paddles != JNI_FALSE {
        flags |= button::PADDLE1 | button::PADDLE2 | button::PADDLE3 | button::PADDLE4;
    }
    if has_share != JNI_FALSE {
        flags |= button::MISC;
    }
    flags |= target_button_flags as u32;
    if has_hat_x != JNI_FALSE {
        flags |= button::LEFT | button::RIGHT;
    }
    if has_hat_y != JNI_FALSE {
        flags |= button::UP | button::DOWN;
    }
    if has_clickpad != JNI_FALSE {
        flags |= button::TOUCHPAD;
    }
    flags as jint
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_computeInputDeviceArrival(
    mut env: JNIEnv,
    _: JClass,
    detected_type: jbyte,
    base_supported_buttons: jint,
    base_capabilities: jshort,
    has_motion: jboolean,
    connect_as: jbyte,
    target_button_flags: jint,
    carries_touchpad_button: jboolean,
    steam_controls: jboolean,
    onscreen_own_player: jboolean,
    controller_number: jshort,
    out: JIntArray,
) {
    let res = compute_input_device_arrival(
        detected_type as u8,
        base_supported_buttons as u32,
        base_capabilities as u16,
        has_motion != JNI_FALSE,
        connect_as as u8,
        target_button_flags as u32,
        carries_touchpad_button != JNI_FALSE,
        steam_controls != JNI_FALSE,
        onscreen_own_player != JNI_FALSE,
        controller_number,
    );
    let flags = i32::from(res.motion_overrides)
        | (i32::from(res.set_needs_clickpad) << 1)
        | (i32::from(res.needs_clickpad_value) << 2);
    write_ints(
        &mut env,
        &out,
        &[
            i32::from(res.reported_type as i8),
            i32::from(res.reported_capabilities as i16),
            res.supported_buttons as i32,
            flags,
        ],
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_formatLaunchController(
    mut env: JNIEnv,
    _: JClass,
    slot: jshort,
    kind: jbyte,
    capabilities: jshort,
    buttons: jint,
) -> jstring {
    let text = launch_controller(slot as u8, kind as u8, capabilities as u16, buttons as u32);
    new_jstring(&mut env, &text)
}
