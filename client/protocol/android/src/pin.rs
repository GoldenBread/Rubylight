//! JNI bridge for `com.limelight.nvstream.jni.NightjarPinJni` (`nightjar-policy::pin` +
//! `nightjar-core::pin_lock`).
//!
//! Caches `Arc<AndroidStore>` per canonical directory so repeated `new PinStore(context)` calls
//! share one background `file_thread`, while supporting uncached handles for restart tests.

mod gate;
mod store;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jbyteArray, jint, jlong, jlongArray, jstring};
use nightjar_core::pin_lock::{Clock, PinGate, PinStore, RingPinCrypto, hasher, throttle};

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

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push(comp.as_os_str());
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn canonical_dir(dir: &Path) -> PathBuf {
    if let Ok(canon) = std::fs::canonicalize(dir) {
        return canon;
    }
    let abs = if dir.is_relative() {
        std::env::current_dir().map_or_else(|_| dir.to_path_buf(), |cwd| cwd.join(dir))
    } else {
        dir.to_path_buf()
    };
    if let Ok(canon) = std::fs::canonicalize(&abs) {
        return canon;
    }
    for normalized in [false, true] {
        let candidate = if normalized {
            normalize_lexical(&abs)
        } else {
            abs.clone()
        };
        let mut cur = candidate.as_path();
        let mut tail: Vec<&OsStr> = Vec::new();
        loop {
            if let Ok(mut canon) = std::fs::canonicalize(cur) {
                for part in tail.into_iter().rev() {
                    canon.push(part);
                }
                return canon;
            }
            let Some(name) = cur.file_name() else {
                break;
            };
            let Some(parent) = cur.parent() else {
                break;
            };
            tail.push(name);
            cur = parent;
        }
        if normalized {
            return candidate;
        }
    }
    abs
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

fn evict_cache(dir: &Path) {
    let key = canonical_dir(dir);
    let raw = dir.to_path_buf();
    let (first, second) = {
        let mut cache = lock(&STORE_CACHE);
        let first = cache.remove(&key);
        let second = if raw != key { cache.remove(&raw) } else { None };
        (first, second)
    };
    if let Some(entry) = first {
        entry.store.flush();
    }
    if let Some(entry) = second {
        entry.store.flush();
    }
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
    let (Ok(a_bytes), Ok(b_bytes)) = (env.convert_byte_array(&a), env.convert_byte_array(&b))
    else {
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
