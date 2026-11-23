//! JNI bridge for `NightjarPinJni.gate*` (`PinGate` pure and stateful policy).

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong};
use nightjar_core::pin_lock::{Mode, PinGate, app_needed, contains, stream_needed, within_grace};

use super::{PIN_GATE, STORE_SLAB, jbool, parse_pcs, read_string};
use crate::slab::lock;

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateAppNeeded(
    mut env: JNIEnv,
    _: JClass,
    pin_set: jboolean,
    mode: JString,
    unlocked: jboolean,
) -> jboolean {
    let mode = read_string(&mut env, &mode)
        .map(|s| Mode::from_str_lossy(&s))
        .unwrap_or(Mode::Off);
    jbool(app_needed(pin_set == JNI_TRUE, mode, unlocked == JNI_TRUE))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateStreamNeeded(
    mut env: JNIEnv,
    _: JClass,
    pin_set: jboolean,
    mode: JString,
    pcs_json: JString,
    uuid: JString,
    unlocked: jboolean,
    grant_valid: jboolean,
) -> jboolean {
    let mode = read_string(&mut env, &mode)
        .map(|s| Mode::from_str_lossy(&s))
        .unwrap_or(Mode::Off);
    let pcs = read_string(&mut env, &pcs_json)
        .map(|s| parse_pcs(&s))
        .unwrap_or_default();
    let uuid = read_string(&mut env, &uuid).unwrap_or_default();
    jbool(stream_needed(
        pin_set == JNI_TRUE,
        mode,
        &pcs,
        &uuid,
        unlocked == JNI_TRUE,
        grant_valid == JNI_TRUE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateContains(
    mut env: JNIEnv,
    _: JClass,
    pcs_json: JString,
    uuid: JString,
) -> jboolean {
    let pcs = read_string(&mut env, &pcs_json)
        .map(|s| parse_pcs(&s))
        .unwrap_or_default();
    let Some(uuid) = read_string(&mut env, &uuid) else {
        return JNI_FALSE;
    };
    jbool(contains(&pcs, &uuid))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateWithinGrace(
    _: JNIEnv,
    _: JClass,
    same_boot: jboolean,
    left_at: jlong,
    now: jlong,
    grace_minutes: jint,
) -> jboolean {
    jbool(within_grace(
        same_boot == JNI_TRUE,
        left_at,
        now,
        u32::try_from(grace_minutes).unwrap_or(0),
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateInstall(
    _: JNIEnv,
    _: JClass,
    store_handle: jlong,
    elapsed: jlong,
    boot: jlong,
) {
    let Some(entry) = STORE_SLAB.get(store_handle) else {
        return;
    };
    entry.clock.update(elapsed, boot);
    let installed = PinGate::new(&entry.store);
    *lock(&PIN_GATE) = installed;
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateNeedsPinForApp(
    _: JNIEnv,
    _: JClass,
    store_handle: jlong,
) -> jboolean {
    let Some(entry) = STORE_SLAB.get(store_handle) else {
        return JNI_FALSE;
    };
    jbool(lock(&PIN_GATE).needs_pin_for_app(&entry.store))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateNeedsPinForStream(
    mut env: JNIEnv,
    _: JClass,
    store_handle: jlong,
    uuid: JString,
    elapsed: jlong,
) -> jboolean {
    let Some(entry) = STORE_SLAB.get(store_handle) else {
        return JNI_FALSE;
    };
    entry.clock.update_elapsed(elapsed);
    let key = read_string(&mut env, &uuid).unwrap_or_default();
    jbool(lock(&PIN_GATE).needs_pin_for_stream(&entry.store, &key))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateStreamLocked(
    mut env: JNIEnv,
    _: JClass,
    store_handle: jlong,
    uuid: JString,
    elapsed: jlong,
) -> jboolean {
    let Some(entry) = STORE_SLAB.get(store_handle) else {
        return JNI_FALSE;
    };
    entry.clock.update_elapsed(elapsed);
    let key = read_string(&mut env, &uuid).unwrap_or_default();
    jbool(lock(&PIN_GATE).locks_stream(&entry.store, &key))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateGrant(
    mut env: JNIEnv,
    _: JClass,
    uuid: JString,
    elapsed: jlong,
) {
    let Some(key) = read_string(&mut env, &uuid) else {
        return;
    };
    lock(&PIN_GATE).grant_at(&key, elapsed);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateForgetGrant(
    _: JNIEnv,
    _: JClass,
) {
    lock(&PIN_GATE).forget_grant();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateUnlockApp(
    _: JNIEnv,
    _: JClass,
) {
    lock(&PIN_GATE).unlock_app();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateFollowFirstScreen(
    _: JNIEnv,
    _: JClass,
) {
    lock(&PIN_GATE).hand_over();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateLeft(
    _: JNIEnv,
    _: JClass,
    store_handle: jlong,
    elapsed: jlong,
    boot: jlong,
) {
    let Some(entry) = STORE_SLAB.get(store_handle) else {
        return;
    };
    entry.clock.update(elapsed, boot);
    lock(&PIN_GATE).left(&entry.store);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateReturned(
    _: JNIEnv,
    _: JClass,
    store_handle: jlong,
    elapsed: jlong,
) {
    let Some(entry) = STORE_SLAB.get(store_handle) else {
        return;
    };
    entry.clock.update_elapsed(elapsed);
    lock(&PIN_GATE).returned(&entry.store);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_gateResetForTests(
    _: JNIEnv,
    _: JClass,
) {
    *lock(&PIN_GATE) = PinGate::empty();
}
