//! JNI exports for `nightjar_policy::controller::mapping`.

use jni::JNIEnv;
use jni::objects::{JClass, JIntArray};
use jni::sys::{JNI_FALSE, jboolean, jbyte, jint, jintArray, jlong};
use nightjar_policy::controller::mapping::{
    ANDROID_TO_LI_BUTTON_MAP, apply_emulated_combos_on_down, apply_emulated_combos_on_up,
    apply_usb_clickpad, button_down_step, button_up_step, flip_face_buttons, held_buttons,
    is_handled_gamepad_key, normalize_mapped_target, remap_key_event,
};

use super::write_ints;
use crate::policy::jbool;

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_androidButtonKeycodes(
    mut env: JNIEnv,
    _: JClass,
) -> jintArray {
    let keys: Vec<i32> = ANDROID_TO_LI_BUTTON_MAP.iter().map(|&(k, _)| k).collect();
    crate::policy::int_array(&mut env, &keys)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_remapKeyEvent(
    mut env: JNIEnv,
    _: JClass,
    back_as_guide: jboolean,
    key_code: jint,
    scan_code: jint,
    flags: jint,
    has_no_modifiers: jboolean,
    vendor_id: jint,
    product_id: jint,
    sdk_int: jint,
    quirk_flags: jint,
    out_state_update: JIntArray,
) -> jint {
    let res = remap_key_event(
        back_as_guide != JNI_FALSE,
        key_code,
        scan_code,
        flags,
        has_no_modifiers != JNI_FALSE,
        vendor_id,
        product_id,
        sdk_int,
        quirk_flags as u32,
    );
    let update = i32::from(res.clear_back_is_start) | (i32::from(res.clear_mode_is_select) << 1);
    write_ints(&mut env, &out_state_update, &[update]);
    res.key_code
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_flipFaceButtonKeyCode(
    _: JNIEnv,
    _: JClass,
    key_code: jint,
) -> jint {
    flip_face_buttons(key_code)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_normalizeMappedTarget(
    _: JNIEnv,
    _: JClass,
    target: jint,
) -> jint {
    normalize_mapped_target(target).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_isHandledGamepadKey(
    _: JNIEnv,
    _: JClass,
    key_code: jint,
    scan_code: jint,
    has_paddles: jboolean,
) -> jboolean {
    jbool(is_handled_gamepad_key(
        key_code,
        scan_code,
        has_paddles != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_buttonDownStep(
    mut env: JNIEnv,
    _: JClass,
    input_map: jint,
    left_trigger: jbyte,
    right_trigger: jbyte,
    has_paddles: jboolean,
    hat_x_used: jboolean,
    hat_y_used: jboolean,
    lt_used: jboolean,
    rt_used: jboolean,
    key_code: jint,
    scan_code: jint,
    out: JIntArray,
) {
    let res = button_down_step(
        input_map as u32,
        left_trigger as u8,
        right_trigger as u8,
        has_paddles != JNI_FALSE,
        hat_x_used != JNI_FALSE,
        hat_y_used != JNI_FALSE,
        lt_used != JNI_FALSE,
        rt_used != JNI_FALSE,
        key_code,
        scan_code,
    );
    let flags = i32::from(res.set_has_select)
        | (i32::from(res.set_has_mode) << 1)
        | (i32::from(res.is_start_or_menu) << 2);
    write_ints(
        &mut env,
        &out,
        &[
            res.status as i32,
            res.input_map as i32,
            i32::from(res.left_trigger),
            i32::from(res.right_trigger),
            flags,
        ],
    );
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_buttonUpStep(
    mut env: JNIEnv,
    _: JClass,
    input_map: jint,
    left_trigger: jbyte,
    right_trigger: jbyte,
    has_paddles: jboolean,
    hat_x_used: jboolean,
    hat_y_used: jboolean,
    lt_used: jboolean,
    rt_used: jboolean,
    key_code: jint,
    scan_code: jint,
    out: JIntArray,
) {
    let res = button_up_step(
        input_map as u32,
        left_trigger as u8,
        right_trigger as u8,
        has_paddles != JNI_FALSE,
        hat_x_used != JNI_FALSE,
        hat_y_used != JNI_FALSE,
        lt_used != JNI_FALSE,
        rt_used != JNI_FALSE,
        key_code,
        scan_code,
    );
    let flags =
        i32::from(res.was_start_held) | (i32::from(res.is_l1) << 1) | (i32::from(res.is_r1) << 2);
    write_ints(
        &mut env,
        &out,
        &[
            res.status as i32,
            res.input_map as i32,
            i32::from(res.left_trigger),
            i32::from(res.right_trigger),
            flags,
        ],
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_heldButtons(
    _: JNIEnv,
    _: JClass,
    input_map: jint,
    emulating_flags: jint,
    has_target_touchpad: jboolean,
    has_select: jboolean,
    has_lb: jboolean,
) -> jint {
    held_buttons(
        input_map as u32,
        emulating_flags as u32,
        has_target_touchpad != JNI_FALSE,
        has_select != JNI_FALSE,
        has_lb != JNI_FALSE,
    ) as jint
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_applyEmulatedCombosOnDown(
    mut env: JNIEnv,
    _: JClass,
    input_map: jint,
    emulating_flags: jint,
    has_select: jboolean,
    has_mode: jboolean,
    needs_clickpad: jboolean,
    event_time_ms: jlong,
    last_lb_up_ms: jlong,
    last_rb_up_ms: jlong,
    out: JIntArray,
) {
    let (map, emu) = apply_emulated_combos_on_down(
        input_map as u32,
        emulating_flags as u32,
        has_select != JNI_FALSE,
        has_mode != JNI_FALSE,
        needs_clickpad != JNI_FALSE,
        event_time_ms,
        last_lb_up_ms,
        last_rb_up_ms,
    );
    write_ints(&mut env, &out, &[map as i32, emu as i32]);
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_applyEmulatedCombosOnUp(
    mut env: JNIEnv,
    _: JClass,
    input_map: jint,
    emulating_flags: jint,
    has_target_touchpad: jboolean,
    has_select: jboolean,
    has_lb: jboolean,
    out: JIntArray,
) {
    let (map, emu) = apply_emulated_combos_on_up(
        input_map as u32,
        emulating_flags as u32,
        has_target_touchpad != JNI_FALSE,
        has_select != JNI_FALSE,
        has_lb != JNI_FALSE,
    );
    write_ints(&mut env, &out, &[map as i32, emu as i32]);
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_applyUsbClickpad(
    mut env: JNIEnv,
    _: JClass,
    buttons: jint,
    clickpad_swallowed: jint,
    device_type: jbyte,
    connect_as: jbyte,
    has_device_motion: jboolean,
    needs_clickpad: jboolean,
    out: JIntArray,
) {
    let (btns, swallowed) = apply_usb_clickpad(
        buttons as u32,
        clickpad_swallowed as u32,
        device_type as u8,
        connect_as as u8,
        has_device_motion != JNI_FALSE,
        needs_clickpad != JNI_FALSE,
    );
    write_ints(&mut env, &out, &[btns as i32, swallowed as i32]);
}
