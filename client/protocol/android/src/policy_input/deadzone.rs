//! JNI exports for `nightjar_policy::controller::deadzone`.

use jni::JNIEnv;
use jni::objects::{JClass, JFloatArray, JIntArray};
use jni::sys::{JNI_FALSE, jboolean, jbyte, jfloat, jint, jshort};
use nightjar_policy::controller::deadzone::{
    AxisSetOutput, apply_stick_deadzone, apply_trigger_deadzone, max_by_magnitude_i16,
    max_by_magnitude_u8, normalize_axis_set, normalize_usb_axes,
};

use super::{write_floats, write_ints};

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_applyStickDeadzone(
    mut env: JNIEnv,
    _: JClass,
    x: jfloat,
    y: jfloat,
    deadzone_radius: jfloat,
    out: JFloatArray,
) {
    let (ox, oy) = apply_stick_deadzone(x, y, deadzone_radius);
    write_floats(&mut env, &out, &[ox, oy]);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_applyTriggerDeadzone(
    _: JNIEnv,
    _: JClass,
    value: jfloat,
    deadzone: jfloat,
) -> jfloat {
    apply_trigger_deadzone(value, deadzone)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_maxByMagnitudeByte(
    _: JNIEnv,
    _: JClass,
    a: jbyte,
    b: jbyte,
) -> jbyte {
    max_by_magnitude_u8(a as u8, b as u8) as jbyte
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_maxByMagnitudeShort(
    _: JNIEnv,
    _: JClass,
    a: jshort,
    b: jshort,
) -> jshort {
    max_by_magnitude_i16(a, b)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_normalizeAxisSet(
    mut env: JNIEnv,
    _: JClass,
    ls_x: jfloat,
    ls_y: jfloat,
    rs_x: jfloat,
    rs_y: jfloat,
    lt: jfloat,
    rt: jfloat,
    hat_x: jfloat,
    hat_y: jfloat,
    has_left_stick: jboolean,
    has_right_stick: jboolean,
    has_triggers: jboolean,
    has_hat: jboolean,
    left_deadzone: jfloat,
    right_deadzone: jfloat,
    trigger_deadzone: jfloat,
    triggers_idle_negative: jboolean,
    left_trigger_used: jboolean,
    right_trigger_used: jboolean,
    hat_x_used: jboolean,
    hat_y_used: jboolean,
    current_input_map: jint,
    out: JIntArray,
) {
    let initial = AxisSetOutput {
        input_map: current_input_map as u32,
        left_trigger_used: left_trigger_used != JNI_FALSE,
        right_trigger_used: right_trigger_used != JNI_FALSE,
        hat_x_used: hat_x_used != JNI_FALSE,
        hat_y_used: hat_y_used != JNI_FALSE,
        ..Default::default()
    };
    let res = normalize_axis_set(
        ls_x,
        ls_y,
        rs_x,
        rs_y,
        lt,
        rt,
        hat_x,
        hat_y,
        has_left_stick != JNI_FALSE,
        has_right_stick != JNI_FALSE,
        has_triggers != JNI_FALSE,
        has_hat != JNI_FALSE,
        left_deadzone,
        right_deadzone,
        trigger_deadzone,
        triggers_idle_negative != JNI_FALSE,
        initial,
    );
    let used_bits = i32::from(res.left_trigger_used)
        | (i32::from(res.right_trigger_used) << 1)
        | (i32::from(res.hat_x_used) << 2)
        | (i32::from(res.hat_y_used) << 3);
    write_ints(
        &mut env,
        &out,
        &[
            res.input_map as i32,
            i32::from(res.left_trigger),
            i32::from(res.right_trigger),
            i32::from(res.left_stick_x),
            i32::from(res.left_stick_y),
            i32::from(res.right_stick_x),
            i32::from(res.right_stick_y),
            used_bits,
        ],
    );
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_normalizeUsbAxes(
    mut env: JNIEnv,
    _: JClass,
    ls_x: jfloat,
    ls_y: jfloat,
    rs_x: jfloat,
    rs_y: jfloat,
    lt: jfloat,
    rt: jfloat,
    left_deadzone: jfloat,
    right_deadzone: jfloat,
    trigger_deadzone: jfloat,
    out: JIntArray,
) {
    let ([lx, ly, rx, ry], [lt_u8, rt_u8]) = normalize_usb_axes(
        ls_x,
        ls_y,
        rs_x,
        rs_y,
        lt,
        rt,
        left_deadzone,
        right_deadzone,
        trigger_deadzone,
    );
    write_ints(
        &mut env,
        &out,
        &[
            i32::from(lx),
            i32::from(ly),
            i32::from(rx),
            i32::from(ry),
            i32::from(lt_u8),
            i32::from(rt_u8),
        ],
    );
}
