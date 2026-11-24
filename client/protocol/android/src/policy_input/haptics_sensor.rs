//! JNI exports for `nightjar_policy::rumble` and `nightjar_policy::controller::haptics_sensor`.

use jni::JNIEnv;
use jni::objects::{JClass, JFloatArray, JIntArray, JLongArray, JString};
use jni::sys::{JNI_FALSE, jboolean, jbyte, jfloat, jint, jlong, jshort};
use nightjar_policy::controller::haptics_sensor::{
    DeviceVibratorAction, GyroLivenessTracker, GyroMapping, MotionFilter, PauseTransition,
    RumbleState, checks_gyro_liveness, clamp_sensor_rate_hz, dual_vibrator_amplitudes,
    evaluate_battery_update, quad_vibrator_amplitudes, sampling_period_us, sce_battery_reading,
    single_vibrator_plan, usb_motion_rate,
};
use nightjar_policy::rumble::{
    RumbleFeel, allows, pwm_timings, scale_motor, shape, shape_amplitude, single_motor_amplitude,
};

use super::{MOTION_FILTER_SLAB, RUMBLE_SLAB, jbool, write_floats, write_ints, write_longs};
use crate::string;

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleAllows(
    _: JNIEnv,
    _: JClass,
    rumble_disabled: jboolean,
) -> jboolean {
    jbool(allows(rumble_disabled != JNI_FALSE))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleFeelFromKey(
    mut env: JNIEnv,
    _: JClass,
    key: JString,
) -> jint {
    let key = string(&mut env, &key);
    RumbleFeel::from_key(key.as_deref()).ordinal()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleShapeMotor(
    _: JNIEnv,
    _: JClass,
    motor: jshort,
    feel_ordinal: jint,
) -> jshort {
    shape(motor as u16, RumbleFeel::from_ordinal(feel_ordinal)) as jshort
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleShapeAmplitude(
    _: JNIEnv,
    _: JClass,
    amplitude: jint,
    feel_ordinal: jint,
) -> jint {
    shape_amplitude(amplitude, RumbleFeel::from_ordinal(feel_ordinal))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleScaleMotor(
    _: JNIEnv,
    _: JClass,
    motor: jshort,
    strength_percent: jint,
) -> jshort {
    scale_motor(motor as u16, strength_percent) as jshort
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleSingleMotorAmplitude(
    _: JNIEnv,
    _: JClass,
    low_freq_motor: jshort,
    high_freq_motor: jshort,
    feel_ordinal: jint,
    strength_percent: jint,
) -> jint {
    single_motor_amplitude(
        low_freq_motor as u16,
        high_freq_motor as u16,
        RumbleFeel::from_ordinal(feel_ordinal),
        strength_percent,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumblePwmTimings(
    mut env: JNIEnv,
    _: JClass,
    amplitude: jint,
    feel_ordinal: jint,
    out: JLongArray,
) {
    let (on, off) = pwm_timings(amplitude, RumbleFeel::from_ordinal(feel_ordinal));
    write_longs(&mut env, &out, &[on, off]);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleStateCreate(
    _: JNIEnv,
    _: JClass,
    allowed: jboolean,
    feel_ordinal: jint,
    device_fallback_slot: jshort,
) -> jlong {
    RUMBLE_SLAB.insert(RumbleState::new(
        allowed != JNI_FALSE,
        RumbleFeel::from_ordinal(feel_ordinal),
        device_fallback_slot,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleStateDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    let _ = RUMBLE_SLAB.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleRecordMotor(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    slot: jshort,
    low: jshort,
    high: jshort,
    stopped: jboolean,
) -> jint {
    let Some(res) = RUMBLE_SLAB.with(handle, |s| {
        s.on_rumble(slot, low as u16, high as u16, stopped != JNI_FALSE)
    }) else {
        return 0;
    };
    i32::from(res.allowed_to_record) | (i32::from(res.active) << 1)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleRecordTriggers(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    slot: jshort,
    lt: jshort,
    rt: jshort,
    stopped: jboolean,
) -> jint {
    let Some(res) = RUMBLE_SLAB.with(handle, |s| {
        s.on_rumble_triggers(slot, lt as u16, rt as u16, stopped != JNI_FALSE)
    }) else {
        return 0;
    };
    i32::from(res.allowed_to_record) | (i32::from(res.active) << 1)
}

fn rumble_set_paused(
    handle: jlong,
    paused: bool,
    stopped: bool,
    buf: &mut [i32; 81],
) -> (jint, usize) {
    let Some(transition) = RUMBLE_SLAB.with(handle, |s| s.set_paused(paused, stopped)) else {
        buf[0] = 0;
        return (0, 1);
    };
    match transition {
        PauseTransition::NoChange => {
            buf[0] = 0;
            (0, 1)
        }
        PauseTransition::Pause { slots, len } => {
            buf[0] = 1;
            for (i, &slot) in slots.iter().enumerate().take(len) {
                buf[1 + i] = i32::from(slot);
            }
            (len as jint, 1 + len)
        }
        PauseTransition::Resume { entries, len } => {
            buf[0] = 2;
            for (i, &(slot, low, high, lt, rt)) in entries.iter().enumerate().take(len) {
                let base = 1 + i * 5;
                buf[base] = i32::from(slot);
                buf[base + 1] = i32::from(low as i16);
                buf[base + 2] = i32::from(high as i16);
                buf[base + 3] = i32::from(lt as i16);
                buf[base + 4] = i32::from(rt as i16);
            }
            (len as jint, 1 + len * 5)
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleSetPaused(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    paused: jboolean,
    stopped: jboolean,
    out: JIntArray,
) -> jint {
    let mut buf = [0i32; 81];
    let (ret, written) =
        rumble_set_paused(handle, paused != JNI_FALSE, stopped != JNI_FALSE, &mut buf);
    write_ints(&mut env, &out, &buf[..written]);
    ret
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumblePlanDual(
    mut env: JNIEnv,
    _: JClass,
    low: jshort,
    high: jshort,
    feel_ordinal: jint,
    out: JIntArray,
) -> jboolean {
    if let Some(amps) = dual_vibrator_amplitudes(
        low as u16,
        high as u16,
        RumbleFeel::from_ordinal(feel_ordinal),
    ) {
        write_ints(&mut env, &out, &[i32::from(amps[0]), i32::from(amps[1])]);
        jbool(true)
    } else {
        jbool(false)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumblePlanQuad(
    mut env: JNIEnv,
    _: JClass,
    low: jshort,
    high: jshort,
    lt: jshort,
    rt: jshort,
    feel_ordinal: jint,
    out: JIntArray,
) -> jboolean {
    if let Some(amps) = quad_vibrator_amplitudes(
        low as u16,
        high as u16,
        lt as u16,
        rt as u16,
        RumbleFeel::from_ordinal(feel_ordinal),
    ) {
        write_ints(
            &mut env,
            &out,
            &[
                i32::from(amps[0]),
                i32::from(amps[1]),
                i32::from(amps[2]),
                i32::from(amps[3]),
            ],
        );
        jbool(true)
    } else {
        jbool(false)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumblePlanSingle(
    mut env: JNIEnv,
    _: JClass,
    low: jshort,
    high: jshort,
    feel_ordinal: jint,
    strength_percent: jint,
    out: JLongArray,
) -> jint {
    let plan = single_vibrator_plan(
        low as u16,
        high as u16,
        RumbleFeel::from_ordinal(feel_ordinal),
        strength_percent,
    );
    write_longs(&mut env, &out, &[plan.pwm_on_ms, plan.pwm_off_ms]);
    jint::from(plan.amplitude)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleDeviceVibratorRoute(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    slot: jshort,
    low: jshort,
    high: jshort,
    found_matching: jboolean,
    vibrated: jboolean,
    osc: jboolean,
    only_l3_r3: jboolean,
    vibrate_osc: jboolean,
    fallback_to_device: jboolean,
    fallback_strength: jint,
    out: JLongArray,
) -> jint {
    let Some(action) = RUMBLE_SLAB.with(handle, |s| {
        s.device_vibrator_route(
            slot,
            low as u16,
            high as u16,
            found_matching != JNI_FALSE,
            vibrated != JNI_FALSE,
            osc != JNI_FALSE,
            only_l3_r3 != JNI_FALSE,
            vibrate_osc != JNI_FALSE,
            fallback_to_device != JNI_FALSE,
            fallback_strength,
        )
    }) else {
        write_longs(&mut env, &out, &[0, 0, 0]);
        return 0;
    };
    match action {
        DeviceVibratorAction::None => 0,
        DeviceVibratorAction::Osc {
            amplitude,
            pwm_on_ms,
            pwm_off_ms,
        } => {
            write_longs(
                &mut env,
                &out,
                &[i64::from(amplitude), pwm_on_ms, pwm_off_ms],
            );
            1
        }
        DeviceVibratorAction::PlayerOneFallback {
            amplitude,
            pwm_on_ms,
            pwm_off_ms,
        } => {
            write_longs(
                &mut env,
                &out,
                &[i64::from(amplitude), pwm_on_ms, pwm_off_ms],
            );
            2
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_rumbleReleaseSlot(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    slot: jshort,
    assigned: jboolean,
    slot_still_assigned: jboolean,
    dev_vib_for_p1: jboolean,
) -> jint {
    let Some((cancel_dev_vib, cleared_slot_levels)) = RUMBLE_SLAB.with(handle, |s| {
        s.on_slot_released(
            slot,
            assigned != JNI_FALSE,
            slot_still_assigned != JNI_FALSE,
            dev_vib_for_p1 != JNI_FALSE,
        )
    }) else {
        return 0;
    };
    i32::from(cancel_dev_vib) | (i32::from(cleared_slot_levels) << 1)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_gyroLivenessStep(
    mut env: JNIEnv,
    _: JClass,
    zero_nanos: jlong,
    x: jfloat,
    y: jfloat,
    z: jfloat,
    timestamp_nanos: jlong,
    state: JLongArray,
) -> jboolean {
    let mut buf = [0i64; 3];
    if state.is_null() || env.get_long_array_region(&state, 0, &mut buf).is_err() {
        return jbool(false);
    }
    let mut tracker = GyroLivenessTracker {
        zero_nanos,
        zero_run_start_nanos: buf[0],
        saw_motion: buf[1] != 0,
        tripped: buf[2] != 0,
    };
    let tripped_now = tracker.on_sample(x, y, z, timestamp_nanos);
    write_longs(
        &mut env,
        &state,
        &[
            tracker.zero_run_start_nanos,
            i64::from(tracker.saw_motion),
            i64::from(tracker.tripped),
        ],
    );
    jbool(tripped_now)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_checksGyroLiveness(
    _: JNIEnv,
    _: JClass,
    slot: jshort,
    device_fallback_slot: jshort,
    using_device_sensors: jboolean,
    fallback_to_device: jboolean,
    has_device_gyro: jboolean,
) -> jboolean {
    jbool(checks_gyro_liveness(
        slot,
        device_fallback_slot,
        using_device_sensors != JNI_FALSE,
        fallback_to_device != JNI_FALSE,
        has_device_gyro != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_clampMotionRateHz(
    _: JNIEnv,
    _: JClass,
    rate_hz: jint,
) -> jint {
    i32::from(clamp_sensor_rate_hz((rate_hz & 0xFFFF) as u16))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_motionSamplingPeriodUs(
    _: JNIEnv,
    _: JClass,
    rate_hz: jint,
) -> jint {
    sampling_period_us((rate_hz & 0xFFFF) as u16)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_usbMotionRateHz(
    _: JNIEnv,
    _: JClass,
    report_rate_hz: jint,
    gamepad_motion_sensors: jboolean,
    drops_motion_and_touchpad: jboolean,
    uses_device_sensors: jboolean,
) -> jint {
    i32::from(usb_motion_rate(
        (report_rate_hz & 0xFFFF) as u16,
        gamepad_motion_sensors != JNI_FALSE,
        drops_motion_and_touchpad != JNI_FALSE,
        uses_device_sensors != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_isValidGyroSensitivity(
    _: JNIEnv,
    _: JClass,
    value: jfloat,
) -> jboolean {
    jbool(GyroMapping::valid_sensitivity(value))
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_validateGyroMapping(
    _: JNIEnv,
    _: JClass,
    x_axis: jint,
    y_axis: jint,
    z_axis: jint,
    x_sign: jint,
    y_sign: jint,
    z_sign: jint,
    h_sens: jfloat,
    v_sens: jfloat,
) -> jint {
    match GyroMapping::validate(
        [x_axis, y_axis, z_axis],
        [x_sign, y_sign, z_sign],
        h_sens,
        v_sens,
    ) {
        Ok(m) => i32::from(m.identity) << 8,
        Err(err) => err as jint,
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_transformGyro(
    mut env: JNIEnv,
    _: JClass,
    x: jfloat,
    y: jfloat,
    z: jfloat,
    x_axis: jint,
    y_axis: jint,
    z_axis: jint,
    x_sign: jint,
    y_sign: jint,
    z_sign: jint,
    h_sens: jfloat,
    v_sens: jfloat,
    out: JFloatArray,
) {
    let mapping = GyroMapping::validate(
        [x_axis, y_axis, z_axis],
        [x_sign, y_sign, z_sign],
        h_sens,
        v_sens,
    )
    .unwrap_or_default();
    let res = mapping.transform([x, y, z]);
    write_floats(&mut env, &out, &res);
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_motionFilterCreate(
    _: JNIEnv,
    _: JClass,
    motion_type: jbyte,
    needs_device_orientation: jboolean,
    check_liveness: jboolean,
    has_mapping: jboolean,
    x_axis: jint,
    y_axis: jint,
    z_axis: jint,
    x_sign: jint,
    y_sign: jint,
    z_sign: jint,
    h_sens: jfloat,
    v_sens: jfloat,
) -> jlong {
    let has_map = has_mapping != JNI_FALSE;
    let mapping = if has_map {
        GyroMapping::validate(
            [x_axis, y_axis, z_axis],
            [x_sign, y_sign, z_sign],
            h_sens,
            v_sens,
        )
        .unwrap_or_default()
    } else {
        GyroMapping::IDENTITY
    };
    MOTION_FILTER_SLAB.insert(MotionFilter::new(
        motion_type as u8,
        needs_device_orientation != JNI_FALSE,
        check_liveness != JNI_FALSE,
        has_map,
        mapping,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_motionFilterDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    let _ = MOTION_FILTER_SLAB.remove(handle);
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_motionSampleStep(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    x: jfloat,
    y: jfloat,
    z: jfloat,
    timestamp_nanos: jlong,
    display_rotation: jint,
    flydigi_forced: jboolean,
    out: JFloatArray,
) -> jint {
    let Some(outcome) = MOTION_FILTER_SLAB.with(handle, |f| {
        f.on_sample(
            [x, y, z],
            timestamp_nanos,
            display_rotation,
            flydigi_forced != JNI_FALSE,
        )
    }) else {
        write_floats(&mut env, &out, &[0.0, 0.0, 0.0]);
        return 0;
    };
    let mut flags = i32::from(outcome.tripped_liveness);
    if let Some(sample) = outcome.send {
        write_floats(&mut env, &out, &sample);
        flags |= 1 << 1;
    }
    flags
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_sceBatteryReading(
    mut env: JNIEnv,
    _: JClass,
    percentage: jint,
    connection_type: jint,
    charging_state: jint,
    out: JFloatArray,
) -> jint {
    let (status, capacity) = sce_battery_reading(percentage, connection_type, charging_state);
    write_floats(&mut env, &out, &[capacity]);
    status
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_evaluateBatteryUpdate(
    mut env: JNIEnv,
    _: JClass,
    send_controllers: jboolean,
    status: jint,
    capacity: jfloat,
    last_status: jint,
    last_capacity: jfloat,
    out: JIntArray,
) -> jboolean {
    let Some((li_state, li_pct)) = evaluate_battery_update(
        send_controllers != JNI_FALSE,
        status,
        capacity,
        last_status,
        last_capacity,
    ) else {
        return jbool(false);
    };
    write_ints(&mut env, &out, &[i32::from(li_state), i32::from(li_pct)]);
    jbool(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_and_destroyed_rumble_handles_zero_pause_output() {
        let mut out = [99i32; 81];
        assert_eq!(rumble_set_paused(0, true, false, &mut out), (0, 1));
        assert_eq!(out[0], 0);

        let handle = RUMBLE_SLAB.insert(RumbleState::new(true, RumbleFeel::Standard, 0));
        assert!(RUMBLE_SLAB.remove(handle));

        let mut destroyed_out = [55i32; 81];
        assert_eq!(
            rumble_set_paused(handle, true, false, &mut destroyed_out),
            (0, 1)
        );
        assert_eq!(destroyed_out[0], 0);
    }
}
