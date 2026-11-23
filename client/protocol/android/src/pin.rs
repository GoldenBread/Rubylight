//! JNI bridge for `com.limelight.nvstream.jni.NightjarPinJni` (`nightjar-policy::pin` +
//! `nightjar-core::pin_lock`).
//!
//! Caches `Arc<AndroidStore>` per canonical directory so repeated `new PinStore(context)` calls
//! share one background `file_thread`, while supporting uncached handles for restart tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jbyteArray, jint, jlong, jlongArray, jstring};
use nightjar_core::pin_lock::{
    Clock, Mode, Outcome, PinGate, PinStore, RingPinCrypto, app_needed, contains, hasher,
    import_legacy_json, stream_needed, throttle, within_grace,
};
use serde_json::json;

use crate::slab::{Slab, lock};

struct AndroidClock {
    elapsed: AtomicI64,
    boot: AtomicI64,
}

impl AndroidClock {
    fn new() -> Self {
        Self {
            elapsed: AtomicI64::new(0),
            boot: AtomicI64::new(0),
        }
    }

    fn update(&self, elapsed: jlong, boot: jlong) {
        self.elapsed.store(elapsed, Ordering::SeqCst);
        self.boot.store(boot, Ordering::SeqCst);
    }

    fn update_elapsed(&self, elapsed: jlong) {
        self.elapsed.store(elapsed, Ordering::SeqCst);
    }
}

impl Clock for AndroidClock {
    fn elapsed(&self) -> i64 {
        self.elapsed.load(Ordering::SeqCst)
    }

    fn boot(&self) -> i64 {
        self.boot.load(Ordering::SeqCst)
    }
}

struct AndroidStore {
    store: PinStore,
    clock: Arc<AndroidClock>,
}

static STORE_CACHE: Mutex<BTreeMap<PathBuf, Arc<AndroidStore>>> = Mutex::new(BTreeMap::new());
static STORE_SLAB: Slab<AndroidStore> = Slab::new();
static PIN_GATE: Mutex<PinGate> = Mutex::new(PinGate::empty());

fn jbool(v: bool) -> jboolean {
    if v { JNI_TRUE } else { JNI_FALSE }
}

fn read_string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

fn new_jstring(env: &JNIEnv, text: &str) -> jstring {
    match env.new_string(text) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn long_array(env: &mut JNIEnv, values: &[jlong]) -> jlongArray {
    let Ok(arr) = env.new_long_array(values.len() as i32) else {
        return std::ptr::null_mut();
    };
    if env.set_long_array_region(&arr, 0, values).is_err() {
        return std::ptr::null_mut();
    }
    arr.into_raw()
}

fn canonical_dir(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

fn open_store(dir: &Path, cached: bool) -> Arc<AndroidStore> {
    let key = canonical_dir(dir);
    if cached {
        let mut cache = lock(&STORE_CACHE);
        if let Some(existing) = cache.get(&key) {
            return existing.clone();
        }
        let clock = Arc::new(AndroidClock::new());
        let store = PinStore::new(&key, clock.clone(), Arc::new(RingPinCrypto));
        let entry = Arc::new(AndroidStore { store, clock });
        cache.insert(key, entry.clone());
        return entry;
    }
    let clock = Arc::new(AndroidClock::new());
    let store = PinStore::new(&key, clock.clone(), Arc::new(RingPinCrypto));
    Arc::new(AndroidStore { store, clock })
}

fn parse_pcs(pcs_json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(pcs_json).unwrap_or_default()
}

// --- PinHasher ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_hasherPbkdf2(
    env: JNIEnv,
    _: JClass,
    password: JByteArray,
    salt: JByteArray,
    iterations: jint,
    key_bytes: jint,
) -> jbyteArray {
    if password.is_null() || salt.is_null() || iterations <= 0 || key_bytes < 0 {
        return std::ptr::null_mut();
    }
    let (Ok(pw), Ok(salt_bytes)) = (
        env.convert_byte_array(&password),
        env.convert_byte_array(&salt),
    ) else {
        return std::ptr::null_mut();
    };
    let Ok(out) = hasher::pbkdf2(&pw, &salt_bytes, iterations as u32, key_bytes as usize) else {
        return std::ptr::null_mut();
    };
    env.byte_array_from_slice(&out)
        .map(|a| a.into_raw())
        .unwrap_or_else(|_| std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_hasherCreate(
    env: JNIEnv,
    _: JClass,
    pin: JByteArray,
    iterations: jint,
) -> jstring {
    if pin.is_null() || iterations <= 0 {
        return std::ptr::null_mut();
    }
    let Ok(bytes) = env.convert_byte_array(&pin) else {
        return std::ptr::null_mut();
    };
    match hasher::create(&bytes, iterations as u32) {
        Ok(record) => new_jstring(&env, &record),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_hasherVerify(
    mut env: JNIEnv,
    _: JClass,
    stored: JString,
    pin: JByteArray,
) -> jboolean {
    let Some(stored) = read_string(&mut env, &stored) else {
        return JNI_FALSE;
    };
    if pin.is_null() {
        return JNI_FALSE;
    }
    let Ok(bytes) = env.convert_byte_array(&pin) else {
        return JNI_FALSE;
    };
    jbool(hasher::verify(&stored, &bytes))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_hasherCalibrate(
    _: JNIEnv,
    _: JClass,
) -> jint {
    hasher::calibrate() as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_hasherScale(
    _: JNIEnv,
    _: JClass,
    probe_nanos: jlong,
) -> jint {
    let nanos = u64::try_from(probe_nanos).unwrap_or(1);
    hasher::scale(nanos) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_hasherConstantTimeEquals(
    env: JNIEnv,
    _: JClass,
    a: JByteArray,
    b: JByteArray,
) -> jboolean {
    if a.is_null() || b.is_null() {
        return jbool(a.is_null() && b.is_null());
    }
    let (Ok(a_bytes), Ok(b_bytes)) = (env.convert_byte_array(&a), env.convert_byte_array(&b)) else {
        return JNI_FALSE;
    };
    jbool(hasher::constant_time_equals(&a_bytes, &b_bytes))
}

// --- PinThrottle ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_throttleDelayAfter(
    _: JNIEnv,
    _: JClass,
    fails: jint,
) -> jlong {
    let fails = u32::try_from(fails).unwrap_or(0);
    throttle::delay_after(fails)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_throttleOnBoot(
    mut env: JNIEnv,
    _: JClass,
    fails: jint,
    lock_until: jlong,
    lock_boot: jlong,
    now: jlong,
    boot: jlong,
) -> jlongArray {
    let stored = throttle::State {
        fails: u32::try_from(fails).unwrap_or(0),
        lock_until,
        lock_boot,
    };
    let next = throttle::on_boot(stored, now, boot, lock_boot == boot);
    let changed = if next == stored { 0 } else { 1 };
    long_array(
        &mut env,
        &[
            jlong::from(next.fails),
            next.lock_until,
            next.lock_boot,
            changed,
        ],
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_throttleRemainingMs(
    _: JNIEnv,
    _: JClass,
    lock_until: jlong,
    now: jlong,
) -> jlong {
    let state = throttle::State {
        fails: 0,
        lock_until,
        lock_boot: 0,
    };
    throttle::remaining_ms(state, now)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPinJni_throttleFailed(
    mut env: JNIEnv,
    _: JClass,
    fails: jint,
    now: jlong,
    boot: jlong,
) -> jlongArray {
    let state = throttle::State {
        fails: u32::try_from(fails).unwrap_or(0),
        lock_until: 0,
        lock_boot: 0,
    };
    let next = throttle::failed(state, now, boot);
    long_array(
        &mut env,
        &[jlong::from(next.fails), next.lock_until, next.lock_boot],
    )
}

// --- PinGate pure & stateful ---

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

// --- PinStore ---

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
    let key = canonical_dir(Path::new(&dir));
    let removed = lock(&STORE_CACHE).remove(&key);
    if let Some(entry) = removed {
        entry.store.flush();
    }
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
    let (Some(entry), Some(pcs_json)) =
        (STORE_SLAB.get(handle), read_string(&mut env, &pcs_json))
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
    let result = entry.store.try_pin_fallible(&pin_bytes, |stored_hash, bytes| {
        if verifier.is_null() {
            return Ok(entry.store.crypto().verify(stored_hash, bytes));
        }
        let hash_arg = if stored_hash.is_empty() {
            JObject::null()
        } else {
            env.new_string(stored_hash).map(JObject::from).map_err(|_| ())?
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
    STORE_SLAB
        .get(handle)
        .map_or(-1, |e| e.store.active_boot())
}
