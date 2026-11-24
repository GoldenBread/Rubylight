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

use crate::native_api;
use crate::policy::{jbool, non_neg_u32, non_neg_u64};
use crate::policy_recovery::new_jstring;
use crate::slab::{MutexSlab, Slab};

struct RoamingEntry {
    tracker: RoamingTracker,
    shift_ms: Option<i64>,
}

fn shifted_u64(shift_ms: &mut Option<i64>, now_ms: jlong) -> u64 {
    let base = *shift_ms.get_or_insert_with(|| now_ms.min(0));
    (i128::from(now_ms) - i128::from(base)).max(0) as u64
}

static ROAMING_SLAB: MutexSlab<RoamingEntry> = MutexSlab::new();
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
    let res = env.with_local_frame(4, |env| {
        let val = env.call_method(target, method, "()Ljava/lang/String;", &[])?;
        let obj = val.l()?;
        if obj.is_null() {
            return Ok::<Option<String>, jni::errors::Error>(None);
        }
        let jstr = JString::from(obj);
        let text: String = env.get_string(&jstr)?.into();
        Ok(Some(text))
    });
    match res {
        Ok(text) => text,
        Err(_) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            None
        }
    }
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
    let res = env.with_local_frame(4, |env| {
        let jmsg = env.new_string(msg)?;
        env.call_method(
            target,
            method,
            "(Ljava/lang/String;)V",
            &[JValue::Object(&jmsg)],
        )?;
        Ok::<(), jni::errors::Error>(())
    });
    if res.is_err() && env.exception_check().unwrap_or(false) {
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
        let res = env.with_local_frame(8, |env| {
            let jhost = env.new_string(&host)?;
            let val = env.call_method(
                &callback,
                "probe",
                "(Ljava/lang/String;I)Ljava/lang/String;",
                &[JValue::Object(&jhost), JValue::Int(port)],
            )?;
            let obj = val.l()?;
            if obj.is_null() {
                return Ok::<Option<String>, jni::errors::Error>(None);
            }
            let jstr = JString::from(obj);
            let msg: String = env.get_string(&jstr)?.into();
            Ok(Some(msg))
        });
        match res {
            Ok(problem) => problem,
            Err(_) => {
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_clear();
                    Some("route probe threw an exception".to_owned())
                } else {
                    Some("JNI route probe failed".to_owned())
                }
            }
        }
    };
    let cancelled = || call_bool_no_arg(&env_cell, &callback, "isCancelled", true);
    let sleep_ms = |ms: u64| -> bool {
        let mut env = env_cell.borrow_mut();
        let clamped = ms.min(i64::MAX as u64) as jlong;
        match env.call_method(&callback, "sleep", "(J)Z", &[JValue::Long(clamped)]) {
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
    ROAMING_SLAB.insert(RoamingEntry {
        tracker: RoamingTracker::new(start),
        shift_ms: None,
    })
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
        .with(handle, |entry| {
            match entry
                .tracker
                .begin_capabilities(network_id as u64, not_metered != JNI_FALSE)
            {
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
            .with(handle, |entry| {
                let shifted = shifted_u64(&mut entry.shift_ms, now_ms);
                entry
                    .tracker
                    .finish_capabilities(
                        network_id as u64,
                        metered != JNI_FALSE,
                        returned != JNI_FALSE,
                        change,
                        shifted,
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
        .with(handle, |entry| {
            let shifted = shifted_u64(&mut entry.shift_ms, now_ms);
            match entry.tracker.begin_addresses(
                network_id as u64,
                &key,
                nat64 != JNI_FALSE,
                shifted,
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
            .with(handle, |entry| {
                entry.tracker.finish_addresses(change).is_some()
            })
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
            .with(handle, |entry| entry.tracker.lost(network_id as u64))
            .unwrap_or(false),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_roamingStop(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    ROAMING_SLAB.with(handle, |entry| entry.tracker.stop());
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_policy::roaming::{NETWORK_CARRIES_ON, RECHECK_MS};
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

    #[test]
    fn roaming_recheck_expires_with_negative_nanotime() {
        let mut entry = RoamingEntry {
            tracker: RoamingTracker::new(Some(1)),
            shift_ms: None,
        };
        assert!(entry.tracker.lost(1));
        assert_eq!(
            entry.tracker.begin_capabilities(2, true),
            CapabilitiesStep::Ask {
                metered: false,
                returned: true
            }
        );
        let t0 = shifted_u64(&mut entry.shift_ms, -50_000);
        assert!(
            entry
                .tracker
                .finish_capabilities(2, false, true, NETWORK_CARRIES_ON, t0)
                .is_some()
        );
        // Before RECHECK_MS (10_000 ms), address change is still asked.
        let t1 = shifted_u64(&mut entry.shift_ms, -45_000);
        assert_eq!(
            entry.tracker.begin_addresses(2, "192.168.1.20", false, t1),
            AddressesStep::Ask { metered: false }
        );
        // At -30_000 (20_000 ms elapsed > RECHECK_MS), recheck must expire.
        let t2 = shifted_u64(&mut entry.shift_ms, -30_000);
        assert!(t2 - t0 > RECHECK_MS);
        assert_eq!(
            entry.tracker.begin_addresses(2, "192.168.1.21", false, t2),
            AddressesStep::Ignore
        );
    }

    #[test]
    fn roaming_recheck_expires_crossing_zero() {
        let mut entry = RoamingEntry {
            tracker: RoamingTracker::new(Some(1)),
            shift_ms: None,
        };
        assert!(entry.tracker.lost(1));
        assert_eq!(
            entry.tracker.begin_capabilities(2, true),
            CapabilitiesStep::Ask {
                metered: false,
                returned: true
            }
        );
        let t0 = shifted_u64(&mut entry.shift_ms, -5_000);
        assert!(
            entry
                .tracker
                .finish_capabilities(2, false, true, NETWORK_CARRIES_ON, t0)
                .is_some()
        );
        // At +4_000 (9_000 ms elapsed <= RECHECK_MS), address change is still asked.
        let t1 = shifted_u64(&mut entry.shift_ms, 4_000);
        assert_eq!(
            entry.tracker.begin_addresses(2, "192.168.1.20", false, t1),
            AddressesStep::Ask { metered: false }
        );
        // Crossing zero from -5_000 to +6_000 (11_000 ms elapsed > RECHECK_MS) must expire.
        let t2 = shifted_u64(&mut entry.shift_ms, 6_000);
        assert!(t2 - t0 > RECHECK_MS);
        assert_eq!(
            entry.tracker.begin_addresses(2, "192.168.1.21", false, t2),
            AddressesStep::Ignore
        );
    }

    #[test]
    fn shifted_u64_handles_extreme_i64_values() {
        let mut shift = None;
        assert_eq!(shifted_u64(&mut shift, i64::MIN), 0);
        assert_eq!(shifted_u64(&mut shift, i64::MIN + 15_000), 15_000);
        assert_eq!(shifted_u64(&mut shift, i64::MAX), u64::MAX);
    }
}
