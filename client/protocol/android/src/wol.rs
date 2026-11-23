//! JNI bridge exports for `NightjarDiscoveryJni` Wake-on-LAN packet building, planning, UDP
//! transmission, `WakeTargetsStore`, `WakeTargetsPollState`, and `WakeLaunch`.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jbyteArray, jint, jintArray, jlong, jstring};
use nightjar_core::wol::{
    Links, Plan, WakeHost, WakeLaunch, WakeStep, WakeTargetEntry, WakeTargetsPollState,
    WakeTargetsStore, build_plan_sockets, magic_packet, parse_ipv4_literal, parse_macs, plan,
    ports, resolve_endpoint_literal, send_udp,
};
use serde::{Deserialize, Serialize};

use crate::http::new_jstring;
use crate::slab::{MutexSlab, Slab};

static TARGET_STORES: Slab<WakeTargetsStore> = Slab::new();
static POLL_STATES: MutexSlab<WakeTargetsPollState> = MutexSlab::new();
static LAUNCHES: MutexSlab<WakeLaunch> = MutexSlab::new();

fn jstr(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

#[derive(Deserialize)]
struct EndpointInput {
    addr: String,
    port: u16,
}

#[derive(Deserialize)]
#[serde(default)]
struct WolPlanRequest {
    macs: Vec<String>,
    endpoints: Vec<EndpointInput>,
    resolved_endpoints: Vec<EndpointInput>,
    wake_broadcasts: Vec<String>,
    local_broadcasts: Vec<String>,
    ipv6_scopes: Vec<u32>,
    fallback_port: u16,
    use_pre_resolved: bool,
}

impl Default for WolPlanRequest {
    fn default() -> Self {
        Self {
            macs: Vec::new(),
            endpoints: Vec::new(),
            resolved_endpoints: Vec::new(),
            wake_broadcasts: Vec::new(),
            local_broadcasts: Vec::new(),
            ipv6_scopes: Vec::new(),
            fallback_port: 47989,
            use_pre_resolved: false,
        }
    }
}

#[derive(Serialize)]
struct SocketDestJson {
    ip: String,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<u32>,
}

#[derive(Serialize)]
struct PlanOkJson {
    payloads_hex: Vec<String>,
    destinations: Vec<SocketDestJson>,
    best_effort: Vec<SocketDestJson>,
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn dest_to_json(addr: &SocketAddr) -> SocketDestJson {
    match addr {
        SocketAddr::V4(v4) => SocketDestJson {
            ip: v4.ip().to_string(),
            port: v4.port(),
            scope: None,
        },
        SocketAddr::V6(v6) => SocketDestJson {
            ip: v6.ip().to_string(),
            port: v6.port(),
            scope: (v6.scope_id() != 0).then_some(v6.scope_id()),
        },
    }
}

fn build_plan_from_request(req: &WolPlanRequest) -> Result<Plan, String> {
    let mut links = Links::default();
    for b in &req.local_broadcasts {
        if let Ok(ip) = b.parse::<Ipv4Addr>() {
            links.broadcasts.insert(ip);
        }
    }
    for &scope in &req.ipv6_scopes {
        links.ipv6.insert(scope);
    }
    if req.use_pre_resolved {
        let mac_refs: Vec<&str> = req.macs.iter().map(String::as_str).collect();
        let resolved: Vec<SocketAddr> = req
            .resolved_endpoints
            .iter()
            .filter_map(|ep| resolve_endpoint_literal(&ep.addr, ep.port))
            .collect();
        let ports: Vec<u16> = if req.endpoints.is_empty() {
            vec![if req.fallback_port == 0 { 47989 } else { req.fallback_port }]
        } else {
            req.endpoints.iter().map(|e| e.port).collect()
        };
        let bcast_refs: Vec<&str> = req.wake_broadcasts.iter().map(String::as_str).collect();
        build_plan_sockets(&mac_refs, &resolved, &ports, &bcast_refs, &links)
    } else {
        let endpoints: Vec<(String, u16)> = req
            .endpoints
            .iter()
            .map(|e| (e.addr.clone(), e.port))
            .collect();
        let primary = req.macs.first().map(String::as_str);
        let extra = if req.macs.len() > 1 {
            &req.macs[1..]
        } else {
            &[]
        };
        let host = WakeHost {
            mac: primary,
            endpoints: &endpoints,
            wake_macs: extra,
            wake_broadcasts: &req.wake_broadcasts,
            own_wake_macs: &[],
            fallback_http_port: if req.fallback_port == 0 {
                47989
            } else {
                req.fallback_port
            },
        };
        plan(&host, &links)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wolMagicPacket(
    mut env: JNIEnv,
    _: JClass,
    mac: JString,
) -> jbyteArray {
    let Some(mac) = jstr(&mut env, &mac) else {
        return std::ptr::null_mut();
    };
    let Ok(packet) = magic_packet(&mac) else {
        return std::ptr::null_mut();
    };
    env.byte_array_from_slice(&packet)
        .map_or(std::ptr::null_mut(), |arr| arr.into_raw())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wolPorts(
    env: JNIEnv,
    _: JClass,
    http_port: jint,
) -> jintArray {
    let list = ports(http_port.clamp(0, u16::MAX as i32) as u16);
    let ints: Vec<jint> = list.into_iter().map(i32::from).collect();
    let Ok(arr) = env.new_int_array(ints.len() as i32) else {
        return std::ptr::null_mut();
    };
    if env.set_int_array_region(&arr, 0, &ints).is_err() {
        return std::ptr::null_mut();
    }
    arr.into_raw()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wolParseMacs(
    mut env: JNIEnv,
    _: JClass,
    text: JString,
) -> jstring {
    let Some(text) = jstr(&mut env, &text) else {
        return std::ptr::null_mut();
    };
    let Some(macs) = parse_macs(&text) else {
        return std::ptr::null_mut();
    };
    let json = serde_json::to_string(&macs).unwrap_or_else(|_| "[]".into());
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wolIpv4Literal(
    mut env: JNIEnv,
    _: JClass,
    literal: JString,
) -> jbyteArray {
    let Some(literal) = jstr(&mut env, &literal) else {
        return std::ptr::null_mut();
    };
    let Some(octets) = parse_ipv4_literal(&literal) else {
        return std::ptr::null_mut();
    };
    env.byte_array_from_slice(&octets)
        .map_or(std::ptr::null_mut(), |arr| arr.into_raw())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wolPlan(
    mut env: JNIEnv,
    _: JClass,
    request_json: JString,
) -> jstring {
    let raw = jstr(&mut env, &request_json).unwrap_or_default();
    let reply = match serde_json::from_str::<WolPlanRequest>(&raw) {
        Ok(req) => match build_plan_from_request(&req) {
            Ok(built) => {
                let ok = PlanOkJson {
                    payloads_hex: built.payloads.iter().map(|p| hex_encode(p)).collect(),
                    destinations: built.destinations.iter().map(dest_to_json).collect(),
                    best_effort: built.best_effort.iter().map(dest_to_json).collect(),
                };
                serde_json::json!({ "ok": ok }).to_string()
            }
            Err(err) => serde_json::json!({ "error": err }).to_string(),
        },
        Err(err) => serde_json::json!({ "error": err.to_string() }).to_string(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wolSend(
    mut env: JNIEnv,
    _: JClass,
    request_json: JString,
) -> jstring {
    let raw = jstr(&mut env, &request_json).unwrap_or_default();
    let err_msg = match serde_json::from_str::<WolPlanRequest>(&raw) {
        Ok(req) => match build_plan_from_request(&req) {
            Ok(built) => match send_udp(&built) {
                Ok(()) => String::new(),
                Err(err) => err,
            },
            Err(err) => err,
        },
        Err(err) => err.to_string(),
    };
    new_jstring(&env, &err_msg)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsOpen(
    mut env: JNIEnv,
    _: JClass,
    dir_path: JString,
) -> jlong {
    let Some(dir) = jstr(&mut env, &dir_path) else {
        return 0;
    };
    TARGET_STORES.insert(WakeTargetsStore::open(dir))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    TARGET_STORES.remove(handle);
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct LegacyWakeImport {
    hosts: BTreeMap<String, WakeTargetEntry>,
    manual: BTreeMap<String, Vec<String>>,
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsImportLegacy(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    legacy_json: JString,
) -> jboolean {
    let Some(store) = TARGET_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let Some(raw) = jstr(&mut env, &legacy_json) else {
        return JNI_FALSE;
    };
    let Ok(parsed) = serde_json::from_str::<LegacyWakeImport>(&raw) else {
        return JNI_FALSE;
    };
    if store
        .import_legacy(parsed.hosts, parsed.manual)
        .unwrap_or(false)
    {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsLoad(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jstring {
    let Some(store) = TARGET_STORES.get(handle) else {
        return std::ptr::null_mut();
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return std::ptr::null_mut();
    };
    let entry = store.load(&uuid);
    if entry.macs.is_empty() && entry.broadcasts.is_empty() {
        return std::ptr::null_mut();
    }
    let json = serde_json::to_string(&entry).unwrap_or_default();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsManual(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jstring {
    let Some(store) = TARGET_STORES.get(handle) else {
        return new_jstring(&env, "[]");
    };
    let list = jstr(&mut env, &uuid)
        .map(|u| store.manual(&u))
        .unwrap_or_default();
    let json = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into());
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsStoreManual(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    macs_json: JString,
) {
    let Some(store) = TARGET_STORES.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let macs: Vec<String> = jstr(&mut env, &macs_json)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let _ = store.store_manual(&uuid, &macs);
}

#[derive(Deserialize)]
struct RawWakeTarget {
    mac: String,
    #[serde(default)]
    broadcasts: Vec<String>,
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsStoreWake(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    primary: JString,
    targets_json: JString,
) {
    let Some(store) = TARGET_STORES.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let primary = jstr(&mut env, &primary);
    let targets: Vec<RawWakeTarget> = jstr(&mut env, &targets_json)
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let _ = store.store_raw(
        &uuid,
        primary.as_deref(),
        targets
            .iter()
            .map(|t| (t.mac.as_str(), t.broadcasts.as_slice())),
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsForget(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) {
    if let (Some(store), Some(uuid)) = (TARGET_STORES.get(handle), jstr(&mut env, &uuid)) {
        let _ = store.forget(&uuid);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsForgetHost(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) {
    if let (Some(store), Some(uuid)) = (TARGET_STORES.get(handle), jstr(&mut env, &uuid)) {
        let _ = store.forget_host(&uuid);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeTargetsHasMac(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    primary_mac: JString,
) -> jboolean {
    let Some(store) = TARGET_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let uuid = jstr(&mut env, &uuid).unwrap_or_default();
    let primary = jstr(&mut env, &primary_mac);
    if store.has_mac(&uuid, primary.as_deref()) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakePollCreate(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    POLL_STATES.insert(WakeTargetsPollState::new())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakePollDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    POLL_STATES.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakePollReserve(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    online: jboolean,
    has_native_api: jboolean,
    now_ms: jlong,
) -> jlong {
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return 0;
    };
    POLL_STATES
        .with(handle, |state| {
            state
                .reserve(
                    &uuid,
                    online == JNI_TRUE,
                    has_native_api == JNI_TRUE,
                    now_ms.max(0) as u64,
                )
                .unwrap_or(0) as jlong
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakePollIsActive(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    token: jlong,
) -> jboolean {
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return JNI_FALSE;
    };
    let active = POLL_STATES
        .with(handle, |state| {
            state.is_active(&uuid, token.max(0) as u64)
        })
        .unwrap_or(false);
    if active { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakePollDrop(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    token: jlong,
) {
    if let Some(uuid) = jstr(&mut env, &uuid) {
        POLL_STATES.with(handle, |state| state.drop_ask(&uuid, token.max(0) as u64));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakePollForget(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) {
    if let Some(uuid) = jstr(&mut env, &uuid) {
        POLL_STATES.with(handle, |state| state.forget(&uuid));
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakePollShutdown(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    POLL_STATES.with(handle, WakeTargetsPollState::shutdown);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakePollPendingCount(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jint {
    POLL_STATES
        .with(handle, |state| state.pending_count() as jint)
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeLaunchCreate(
    _: JNIEnv,
    _: JClass,
    start_ms: jlong,
    timeout_ms: jlong,
    resend_ms: jlong,
) -> jlong {
    LAUNCHES.insert(WakeLaunch::with(
        start_ms.max(0) as u64,
        timeout_ms.max(0) as u64,
        resend_ms.max(0) as u64,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeLaunchDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    LAUNCHES.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_wakeLaunchNext(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    online: jboolean,
    now_ms: jlong,
) -> jint {
    let step = LAUNCHES
        .with(handle, |launch| {
            launch.next(online == JNI_TRUE, now_ms.max(0) as u64)
        })
        .unwrap_or(WakeStep::TimedOut);
    match step {
        WakeStep::Wait => 0,
        WakeStep::Send => 1,
        WakeStep::Online => 2,
        WakeStep::TimedOut => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn wol_plan_preserves_scoped_ipv6_resolved_endpoints() {
        let req = WolPlanRequest {
            macs: vec!["02:7c:16:b4:13:4f".into()],
            endpoints: vec![EndpointInput {
                addr: "fe80::1%wlan0".into(),
                port: 47989,
            }],
            resolved_endpoints: vec![
                EndpointInput {
                    addr: "fe80::1%wlan0".into(),
                    port: 47989,
                },
                EndpointInput {
                    addr: "fe80::2%3".into(),
                    port: 47989,
                },
            ],
            use_pre_resolved: true,
            ..Default::default()
        };
        let built = build_plan_from_request(&req).unwrap();
        let v6_1: Ipv6Addr = "fe80::1".parse().unwrap();
        let v6_2: Ipv6Addr = "fe80::2".parse().unwrap();
        assert!(built.destinations.iter().any(|d| matches!(
            d,
            SocketAddr::V6(v6) if *v6.ip() == v6_1 && v6.port() == 47998
        )));
        assert!(built.destinations.iter().any(|d| matches!(
            d,
            SocketAddr::V6(v6) if *v6.ip() == v6_2 && v6.scope_id() == 3 && v6.port() == 47998
        )));
    }
}
