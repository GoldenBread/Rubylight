//! JNI bridge for `NightjarJni` host-loss watch, route checks, and `NightjarPolicyJni`
//! roaming tracker (`nightjar_core::{host_loss, route}` and `nightjar_policy::roaming`).

use std::cell::RefCell;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};

use jni::JNIEnv;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{JNI_FALSE, jboolean, jint, jlong, jstring};
use nightjar_core::host_loss::{Goodbye, watch_goodbye};
use nightjar_core::route::{RouteWaitOutcome, route_problem, wait_for_route};
use nightjar_policy::recovery::{PEER_TIMEOUT_MS, reconnects_at_once};
use nightjar_policy::roaming::{AddressesStep, CapabilitiesStep, RoamingTracker};

use crate::engine;
use crate::native_api;
use crate::policy::{jbool, non_neg_u32, non_neg_u64};
use crate::policy_recovery::new_jstring;
use crate::slab::{MutexSlab, Slab};

static ROAMING_SLAB: MutexSlab<RoamingTracker> = MutexSlab::new();
static HOST_LOSS_STOP_SLAB: Slab<AtomicBool> = Slab::new();

fn parse_ip(env: &mut JNIEnv, text: &JString) -> Option<IpAddr> {
    native_api::string(env, text).and_then(|s| s.parse().ok())
}

fn call_string_no_arg(
    env: &RefCell<JNIEnv<'_>>,
    target: &JObject<'_>,
    method: &str,
) -> Option<String> {
    let mut env = env.borrow_mut();
    let res = env.call_method(target, method, "()Ljava/lang/String;", &[]);
    let Ok(val) = res else {
        let _ = env.exception_clear();
        return None;
    };
    let Ok(obj) = val.l() else {
        return None;
    };
    if obj.is_null() {
        return None;
    }
    let jstr = JString::from(obj);
    env.get_string(&jstr).ok().map(Into::into)
}

fn call_bool_no_arg(
    env: &RefCell<JNIEnv<'_>>,
    target: &JObject<'_>,
    method: &str,
    on_err: bool,
) -> bool {
    let mut env = env.borrow_mut();
    let res = env.call_method(target, method, "()Z", &[]);
    match res {
        Ok(v) => v.z().unwrap_or(on_err),
        Err(_) => {
            let _ = env.exception_clear();
            on_err
        }
    }
}

fn call_void_str(env: &RefCell<JNIEnv<'_>>, target: &JObject<'_>, method: &str, msg: &str) {
    let mut env = env.borrow_mut();
    let Ok(jmsg) = env.new_string(msg) else {
        return;
    };
    if env
        .call_method(
            target,
            method,
            "(Ljava/lang/String;)V",
            &[JValue::Object(&jmsg)],
        )
        .is_err()
    {
        let _ = env.exception_clear();
    }
}

// --- HostLossWatch & Route (NightjarJni) ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_hostLossWatchCreate(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    HOST_LOSS_STOP_SLAB.insert(AtomicBool::new(false))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_hostLossWatchStop(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(stop) = HOST_LOSS_STOP_SLAB.get(handle) {
        stop.store(true, Ordering::Release);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_hostLossWatchDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    HOST_LOSS_STOP_SLAB.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_hostLossWatchGoodbye(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    host: JString,
    port: jint,
    callback: JObject,
) -> jint {
    let stop = HOST_LOSS_STOP_SLAB.get(handle);
    if stop.as_ref().is_some_and(|s| s.load(Ordering::Acquire)) {
        return Goodbye::Unknown.code();
    }
    let Some(ip) = parse_ip(&mut env, &host) else {
        return Goodbye::Unknown.code();
    };
    let Ok(port) = u16::try_from(port) else {
        return Goodbye::Unknown.code();
    };
    if callback.is_null() {
        return Goodbye::Unknown.code();
    }
    let env_cell = RefCell::new(env);
    let gateway = |_: IpAddr| -> Option<IpAddr> {
        call_string_no_arg(&env_cell, &callback, "gatewayToward")
            .and_then(|s| s.parse::<IpAddr>().ok())
    };
    let stopped = || -> bool {
        stop.as_ref().is_some_and(|s| s.load(Ordering::Acquire))
            || call_bool_no_arg(&env_cell, &callback, "isStopped", true)
    };
    let log = |line: &str| call_void_str(&env_cell, &callback, "log", line);
    let _ = engine::with_session(false, |s| s.host_said_goodbye());
    watch_goodbye(SocketAddr::new(ip, port), &gateway, &stopped, &log).code()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_routeProblem(
    mut env: JNIEnv,
    _: JClass,
    host: JString,
    port: jint,
) -> jstring {
    let Some(host) = native_api::string(&mut env, &host) else {
        return std::ptr::null_mut();
    };
    let port = u16::try_from(port).unwrap_or(47989);
    match route_problem(&host, port) {
        Some(msg) => new_jstring(&env, &msg),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_routeWait(
    mut env: JNIEnv,
    _: JClass,
    host: JString,
    port: jint,
    deadline_ms: jlong,
    callback: JObject,
) -> jint {
    let Some(host) = native_api::string(&mut env, &host) else {
        return 0;
    };
    if callback.is_null() {
        return 2;
    }
    let env_cell = RefCell::new(env);
    let now_ms = || -> u64 {
        let mut env = env_cell.borrow_mut();
        match env.call_method(&callback, "nowMs", "()J", &[]) {
            Ok(v) => v.j().unwrap_or(0).max(0) as u64,
            Err(_) => {
                let _ = env.exception_clear();
                0
            }
        }
    };
    let probe = || -> Option<String> {
        let mut env = env_cell.borrow_mut();
        let Ok(jhost) = env.new_string(&host) else {
            return Some("JNI string allocation failed".into());
        };
        let res = env.call_method(
            &callback,
            "probe",
            "(Ljava/lang/String;I)Ljava/lang/String;",
            &[JValue::Object(&jhost), JValue::Int(port)],
        );
        let Ok(val) = res else {
            let _ = env.exception_clear();
            return None;
        };
        let Ok(obj) = val.l() else {
            return None;
        };
        if obj.is_null() {
            return None;
        }
        let jstr = JString::from(obj);
        env.get_string(&jstr).ok().map(Into::into)
    };
    let cancelled = || call_bool_no_arg(&env_cell, &callback, "isCancelled", true);
    let sleep_ms = |ms: u64| -> bool {
        let mut env = env_cell.borrow_mut();
        match env.call_method(&callback, "sleep", "(J)Z", &[JValue::Long(ms as jlong)]) {
            Ok(v) => v.z().unwrap_or(false),
            Err(_) => {
                let _ = env.exception_clear();
                false
            }
        }
    };
    let log_info = |line: &str| call_void_str(&env_cell, &callback, "logInfo", line);
    let log_warn = |line: &str| call_void_str(&env_cell, &callback, "logWarn", line);
    match wait_for_route(
        non_neg_u64(deadline_ms),
        &now_ms,
        &probe,
        &cancelled,
        &sleep_ms,
        &log_info,
        &log_warn,
    ) {
        RouteWaitOutcome::Reachable => 0,
        RouteWaitOutcome::Exhausted => 1,
        RouteWaitOutcome::Cancelled => 2,
    }
}

// --- RouteWait (value-tuple JNI) ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryPeerTimeoutMs(
    _: JNIEnv,
    _: JClass,
) -> jint {
    PEER_TIMEOUT_MS as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryReconnectsAtOnce(
    _: JNIEnv,
    _: JClass,
    cannot_continue: jboolean,
    metered: jboolean,
    mobile_allowed: jboolean,
    auto_reconnect: jboolean,
    network_grace_ms: jint,
) -> jboolean {
    jbool(reconnects_at_once(
        cannot_continue != JNI_FALSE,
        metered != JNI_FALSE,
        mobile_allowed != JNI_FALSE,
        auto_reconnect != JNI_FALSE,
        non_neg_u32(network_grace_ms),
    ))
}

// --- Roaming (nightjar_policy::roaming) ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_roamingCreate(
    _: JNIEnv,
    _: JClass,
    has_stream_network: jboolean,
    stream_network_id: jlong,
) -> jlong {
    let start = (has_stream_network != JNI_FALSE).then_some(stream_network_id as u64);
    ROAMING_SLAB.insert(RoamingTracker::new(start))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_roamingDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    ROAMING_SLAB.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_roamingOnCapabilities(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    network_id: jlong,
    not_metered: jboolean,
) -> jint {
    ROAMING_SLAB
        .with(handle, |t| {
            match t.begin_capabilities(network_id as u64, not_metered != JNI_FALSE) {
                CapabilitiesStep::Ignore => 0,
                CapabilitiesStep::Ask {
                    metered: false,
                    returned: false,
                } => 1,
                CapabilitiesStep::Ask {
                    metered: true,
                    returned: false,
                } => 2,
                CapabilitiesStep::Ask {
                    metered: false,
                    returned: true,
                } => 3,
                CapabilitiesStep::Ask {
                    metered: true,
                    returned: true,
                } => 4,
            }
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_roamingRecordChange(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    network_id: jlong,
    metered: jboolean,
    returned: jboolean,
    change: jint,
    now_ms: jlong,
) -> jboolean {
    jbool(
        ROAMING_SLAB
            .with(handle, |t| {
                t.finish_capabilities(
                    network_id as u64,
                    metered != JNI_FALSE,
                    returned != JNI_FALSE,
                    change,
                    non_neg_u64(now_ms),
                )
                .is_some()
            })
            .unwrap_or(false),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_roamingOnAddresses(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    network_id: jlong,
    addresses_key: JString,
    nat64: jboolean,
    now_ms: jlong,
) -> jint {
    let key = native_api::string(&mut env, &addresses_key).unwrap_or_default();
    ROAMING_SLAB
        .with(handle, |t| {
            match t.begin_addresses(
                network_id as u64,
                &key,
                nat64 != JNI_FALSE,
                non_neg_u64(now_ms),
            ) {
                AddressesStep::Ignore => -1,
                AddressesStep::Ask { metered: false } => 0,
                AddressesStep::Ask { metered: true } => 1,
            }
        })
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_roamingRecordAddressChange(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    change: jint,
) -> jboolean {
    jbool(
        ROAMING_SLAB
            .with(handle, |t| t.finish_addresses(change).is_some())
            .unwrap_or(false),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_roamingOnLost(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    network_id: jlong,
) -> jboolean {
    jbool(
        ROAMING_SLAB
            .with(handle, |t| t.lost(network_id as u64))
            .unwrap_or(false),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_roamingStop(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    ROAMING_SLAB.with(handle, RoamingTracker::stop);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

    #[test]
    fn route_problem_and_stop_token_work() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(route_problem("127.0.0.1", port), None);

        let h = HOST_LOSS_STOP_SLAB.insert(AtomicBool::new(false));
        assert!(!HOST_LOSS_STOP_SLAB.get(h).unwrap().load(Ordering::Acquire));
        HOST_LOSS_STOP_SLAB
            .get(h)
            .unwrap()
            .store(true, Ordering::Release);
        assert!(HOST_LOSS_STOP_SLAB.get(h).unwrap().load(Ordering::Acquire));
        assert!(HOST_LOSS_STOP_SLAB.remove(h));
    }
}
