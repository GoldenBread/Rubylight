//! JNI bridge for `NightjarPinJni.store*` (`PinStore` persistence, legacy import, and try-lock).

use std::path::Path;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{JNI_TRUE, jboolean, jint, jlong, jlongArray, jstring};
use nightjar_core::pin_lock::{Mode, Outcome, RingPinCrypto, import_legacy_json};
use serde_json::json;

use super::{
    STORE_CACHE, STORE_SLAB, canonical_dir, evict_cache, long_array, new_jstring, open_store,
    parse_pcs, read_string,
};
use crate::slab::lock;

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeOpen(
    mut env: JNIEnv,
    _: JClass,
    dir: JString,
    cached: jboolean,
) -> jlong {
    let Some(dir) = read_string(&mut env, &dir) else {
        return 0;
    };
    let entry = open_store(Path::new(&dir), cached == JNI_TRUE);
    STORE_SLAB.insert_arc(entry)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    STORE_SLAB.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeFlush(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(entry) = STORE_SLAB.get(handle) {
        entry.store.flush();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeEvictCacheForTests(
    mut env: JNIEnv,
    _: JClass,
    dir: JString,
) {
    let Some(dir) = read_string(&mut env, &dir) else {
        return;
    };
    evict_cache(Path::new(&dir));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeImportLegacy(
    mut env: JNIEnv,
    _: JClass,
    dir: JString,
    legacy_json: JString,
) -> jint {
    let (Some(dir), Some(json)) = (
        read_string(&mut env, &dir),
        read_string(&mut env, &legacy_json),
    ) else {
        return -1;
    };
    let key = canonical_dir(Path::new(&dir));
    match import_legacy_json(&key, &json, &RingPinCrypto) {
        Ok(true) => {
            if let Some(existing) = lock(&STORE_CACHE).get(&key).cloned() {
                existing.store.reload_now();
            }
            1
        }
        Ok(false) => 0,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeViewJson(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let Some(entry) = STORE_SLAB.get(handle) else {
        return std::ptr::null_mut();
    };
    let view = entry.store.view();
    let len = entry.store.pin_length();
    let problem = entry.store.problem().is_some();
    let json = json!({
        "has_pin": view.has_pin,
        "pin_length": len,
        "mode": view.mode.as_str(),
        "pcs": view.pcs,
        "grace_minutes": view.grace_minutes,
        "problem": problem,
    })
    .to_string();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeSetMode(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    mode: JString,
) {
    let (Some(entry), Some(mode)) = (STORE_SLAB.get(handle), read_string(&mut env, &mode)) else {
        return;
    };
    entry.store.set_mode(Mode::from_str_lossy(&mode));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeSetPcs(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    pcs_json: JString,
) {
    let (Some(entry), Some(pcs_json)) = (STORE_SLAB.get(handle), read_string(&mut env, &pcs_json))
    else {
        return;
    };
    entry.store.set_pcs(parse_pcs(&pcs_json));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeSetGraceMinutes(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    minutes: jint,
) {
    let Some(entry) = STORE_SLAB.get(handle) else {
        return;
    };
    entry
        .store
        .set_grace_minutes(u32::try_from(minutes).unwrap_or(0));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeSetPin(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    pin: JByteArray,
    iterations: jint,
) -> jstring {
    let Some(entry) = STORE_SLAB.get(handle) else {
        return new_jstring(&env, "invalid store handle");
    };
    if pin.is_null() {
        return new_jstring(&env, "null PIN");
    }
    let Ok(bytes) = env.convert_byte_array(&pin) else {
        return new_jstring(&env, "invalid PIN bytes");
    };
    let result = if iterations > 0 {
        entry.store.set_pin_with(&bytes, iterations as u32)
    } else {
        entry.store.set_pin(&bytes)
    };
    match result {
        Ok(()) => std::ptr::null_mut(),
        Err(e) => new_jstring(&env, &e),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeRemovePin(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    let Some(entry) = STORE_SLAB.get(handle) else {
        return;
    };
    entry.store.remove_pin();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeLockedForMs(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    elapsed: jlong,
    boot: jlong,
) -> jlong {
    let Some(entry) = STORE_SLAB.get(handle) else {
        return 0;
    };
    entry.clock.update(elapsed, boot);
    entry.store.locked_for_ms()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeTryPin(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    pin: JByteArray,
    elapsed: jlong,
    boot: jlong,
    verifier: JObject,
) -> jlongArray {
    let Some(entry) = STORE_SLAB.get(handle) else {
        return long_array(&mut env, &[3, 0]);
    };
    if pin.is_null() {
        return long_array(&mut env, &[1, 0]);
    }
    let Ok(pin_bytes) = env.convert_byte_array(&pin) else {
        return long_array(&mut env, &[1, 0]);
    };
    entry.clock.update(elapsed, boot);
    let result = entry
        .store
        .try_pin_fallible(&pin_bytes, |stored_hash, bytes| {
            if verifier.is_null() {
                return Ok(entry.store.crypto().verify(stored_hash, bytes));
            }
            let hash_arg = if stored_hash.is_empty() {
                JObject::null()
            } else {
                env.new_string(stored_hash)
                    .map(JObject::from)
                    .map_err(|_| ())?
            };
            let pin_arg = env
                .byte_array_from_slice(bytes)
                .map(JObject::from)
                .map_err(|_| ())?;
            match env.call_method(
                &verifier,
                "matches",
                "(Ljava/lang/String;[B)Z",
                &[JValue::Object(&hash_arg), JValue::Object(&pin_arg)],
            ) {
                Ok(val) => val.z().map_err(|_| ()),
                Err(_) => Err(()),
            }
        });
    let Ok(outcome) = result else {
        // Pending Java exception from PinVerifier.matches propagates directly to JVM.
        return std::ptr::null_mut();
    };
    let pair = match outcome {
        Outcome::Ok => [0, 0],
        Outcome::Wrong { locked_for_ms } => [1, locked_for_ms],
        Outcome::Locked { locked_for_ms } => [2, locked_for_ms],
        Outcome::Unavailable => [3, 0],
    };
    long_array(&mut env, &pair)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeSetActive(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    elapsed: jlong,
    boot: jlong,
) {
    let Some(entry) = STORE_SLAB.get(handle) else {
        return;
    };
    entry.store.set_active(elapsed, boot);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeActiveAt(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jlong {
    STORE_SLAB.get(handle).map_or(0, |e| e.store.active_at())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_storeActiveBoot(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jlong {
    STORE_SLAB.get(handle).map_or(-1, |e| e.store.active_boot())
}
