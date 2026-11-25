//! JNI bridge exports for `NightjarHostsJni` unified [`HostManager`], `HostClientPool`,
//! parallel direct/internet polling, address pinning, certificate management, and STUN helpers.

use std::collections::VecDeque;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};
use nightjar_core::hosts::manager::{HostManager, HostSnapshot, LinkAddOutcome};
use nightjar_core::hosts::HostEntry;
use nightjar_core::http::ClientIdentity;
use nightjar_policy::hosts::{EndpointSlot, HostReachState};
use serde::{Deserialize, Serialize};

use crate::http::{new_jstring, server_info_json};
use crate::slab::{Slab, lock};

use super::{JniHostDto, dto_to_entry, entry_to_dto, jstr};

pub(crate) struct ManagedHostBridge {
    pub(crate) manager: HostManager,
    events: Arc<(Mutex<VecDeque<HostSnapshot>>, Condvar)>,
    closed: Arc<AtomicBool>,
}

impl ManagedHostBridge {
    fn shutdown_nonblocking(&self) {
        {
            let _q = lock(&self.events.0);
            self.closed.store(true, Ordering::Release);
            self.events.1.notify_all();
        }
        self.manager.shutdown();
    }
}

pub(crate) static HOST_MANAGERS: Slab<ManagedHostBridge> = Slab::new();
static PENDING_DESTROYS: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());

#[derive(Default, Deserialize)]
#[serde(default)]
struct JniHostInputDto {
    #[serde(flatten)]
    host: JniHostDto,
    alias: Option<String>,
    https_port: u16,
    pinned: Option<EndpointSlot>,
    internet_play: Option<String>,
}

fn input_to_entry(dto: JniHostInputDto) -> HostEntry {
    let mut entry = dto_to_entry(dto.host);
    if dto.alias.is_some() {
        entry.shared.alias = dto.alias;
    }
    if dto.https_port != 0 {
        entry.shared.https_port = dto.https_port;
    }
    if dto.pinned.is_some() {
        entry.pinned_endpoint = dto.pinned;
    }
    if dto.internet_play.is_some() {
        entry.shared.internet_play = dto.internet_play;
    }
    entry.sync_shared_from_slots();
    entry
}

#[derive(Serialize)]
struct JniSnapshotDto {
    #[serde(flatten)]
    entry: JniHostDto,
    state: &'static str,
    active: Option<EndpointSlot>,
    pinned: Option<EndpointSlot>,
    alias: Option<String>,
    paired: bool,
    another_certificate: bool,
    running_game_id: i32,
    running_game_uuid: Option<String>,
    https_port: u16,
    external_port: u16,
    unreachable: Option<String>,
    capabilities_json: Option<String>,
    internet_path: Option<String>,
    needs_vpn_stun: bool,
    server_info: Option<serde_json::Value>,
}

fn snapshot_to_dto(s: &HostSnapshot) -> JniSnapshotDto {
    JniSnapshotDto {
        entry: entry_to_dto(&s.entry),
        state: match s.state {
            HostReachState::Unknown => "UNKNOWN",
            HostReachState::Online => "ONLINE",
            HostReachState::Offline => "OFFLINE",
        },
        active: s.active_endpoint.clone(),
        pinned: s.pinned_endpoint.clone(),
        alias: s.alias.clone(),
        paired: s.paired,
        another_certificate: s.another_certificate,
        running_game_id: s.running_game_id,
        running_game_uuid: s.running_game_uuid.clone(),
        https_port: s.entry.shared.https_port,
        external_port: s.external_port,
        unreachable: s.unreachable.clone(),
        capabilities_json: s.capabilities_json.clone(),
        internet_path: s.internet_path.clone(),
        needs_vpn_stun: s.needs_vpn_stun,
        server_info: s.server_info.as_ref().map(server_info_json),
    }
}

fn jbytes(env: &JNIEnv, arr: &JByteArray) -> Option<Vec<u8>> {
    if arr.is_null() {
        return None;
    }
    env.convert_byte_array(arr).ok()
}

fn jb(v: bool) -> jboolean {
    if v { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerOpen(
    mut env: JNIEnv,
    _: JClass,
    config_dir: JString,
    unique_id: JString,
    cert_pem: JString,
    key_pem: JString,
) -> jlong {
    let Some(dir) = jstr(&mut env, &config_dir) else {
        return 0;
    };
    let id = match (
        jstr(&mut env, &unique_id),
        jstr(&mut env, &cert_pem),
        jstr(&mut env, &key_pem),
    ) {
        (Some(uid), Some(cert), Some(key)) if !uid.is_empty() => {
            ClientIdentity::from_pem(&uid, &cert, &key).ok().map(Arc::new)
        }
        _ => None,
    };
    let Ok(manager) = HostManager::open(std::path::Path::new(&dir), id) else {
        return 0;
    };
    let events: Arc<(Mutex<VecDeque<HostSnapshot>>, Condvar)> =
        Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
    let closed = Arc::new(AtomicBool::new(false));
    let events_cb = Arc::clone(&events);
    let closed_cb = Arc::clone(&closed);
    manager.set_listener(Some(Arc::new(move |snap| {
        if closed_cb.load(Ordering::Acquire) {
            return;
        }
        let mut q = lock(&events_cb.0);
        if q.len() >= 256 {
            q.pop_front();
        }
        q.push_back(snap.clone());
        events_cb.1.notify_all();
    })));
    HOST_MANAGERS.insert(ManagedHostBridge {
        manager,
        events,
        closed,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    let Some(bridge) = HOST_MANAGERS.take(handle) else {
        return;
    };
    bridge.shutdown_nonblocking();
    if bridge.manager.has_active_workers() {
        let reaper = std::thread::Builder::new()
            .name("nj-hm-reaper".into())
            .spawn(move || {
                bridge.manager.wait_for_polling_stopped();
            })
            .ok();
        if let Some(reaper_handle) = reaper {
            let mut pending = lock(&PENDING_DESTROYS);
            pending.retain(|h| !h.is_finished());
            pending.push(reaper_handle);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerResetForTests(
    _: JNIEnv,
    _: JClass,
) {
    for bridge in HOST_MANAGERS.drain_all() {
        bridge.shutdown_nonblocking();
        bridge.manager.wait_for_polling_stopped();
    }
    let pending = {
        let mut guard = lock(&PENDING_DESTROYS);
        guard.drain(..).collect::<Vec<_>>()
    };
    for handle in pending {
        let _ = handle.join();
    }
    super::hosts_poller::drain_all_pollers();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerStartPolling(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(bridge) = HOST_MANAGERS.get(handle) {
        bridge.manager.start_polling();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerStopPolling(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(bridge) = HOST_MANAGERS.get(handle) {
        bridge.manager.stop_polling();
        let _q = lock(&bridge.events.0);
        bridge.events.1.notify_all();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerWaitForPollingStopped(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(bridge) = HOST_MANAGERS.get(handle) {
        bridge.manager.wait_for_polling_stopped();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerNowMs(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jlong {
    HOST_MANAGERS
        .get(handle)
        .map_or(0, |b| b.manager.now_ms() as jlong)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerNextEvent(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    timeout_ms: jlong,
) -> jstring {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return ptr::null_mut();
    };
    let (lock_q, cvar) = &*bridge.events;
    let mut q = lock(lock_q);
    if q.is_empty() && !bridge.closed.load(Ordering::Acquire) && timeout_ms > 0 {
        let (guard, _) = cvar
            .wait_timeout_while(q, Duration::from_millis(timeout_ms as u64), |queue| {
                queue.is_empty() && !bridge.closed.load(Ordering::Acquire)
            })
            .unwrap_or_else(|e| e.into_inner());
        q = guard;
    }
    let Some(snap) = q.pop_front() else {
        return ptr::null_mut();
    };
    let json = serde_json::to_string(&snapshot_to_dto(&snap)).unwrap_or_default();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerGetSnapshot(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jstring {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return ptr::null_mut();
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return ptr::null_mut();
    };
    let Some(snap) = bridge.manager.get_snapshot(&uuid) else {
        return ptr::null_mut();
    };
    let json = serde_json::to_string(&snapshot_to_dto(&snap)).unwrap_or_default();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerAddComputerBlocking(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    dto_json: JString,
) -> jstring {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return ptr::null_mut();
    };
    let Some(raw) = jstr(&mut env, &dto_json) else {
        return ptr::null_mut();
    };
    let Ok(dto) = serde_json::from_str::<JniHostInputDto>(&raw) else {
        return ptr::null_mut();
    };
    let Some(snap) = bridge.manager.add_computer_blocking(input_to_entry(dto)) else {
        return ptr::null_mut();
    };
    let json = serde_json::to_string(&snapshot_to_dto(&snap)).unwrap_or_default();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerAddComputerForLink(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    dto_json: JString,
    link_cert_sha256: JByteArray,
) -> jstring {
    const UNREACHABLE: &str = r#"{"outcome":1,"snapshot":null}"#;
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return new_jstring(&env, UNREACHABLE);
    };
    let Some(raw) = jstr(&mut env, &dto_json) else {
        return new_jstring(&env, UNREACHABLE);
    };
    let Ok(dto) = serde_json::from_str::<JniHostInputDto>(&raw) else {
        return new_jstring(&env, UNREACHABLE);
    };
    let Some(sha_vec) = jbytes(&env, &link_cert_sha256) else {
        return new_jstring(&env, UNREACHABLE);
    };
    let Ok(sha) = <[u8; 32]>::try_from(sha_vec) else {
        return new_jstring(&env, UNREACHABLE);
    };
    let (outcome, snap) = bridge.manager.add_computer_for_link(input_to_entry(dto), sha);
    let code = match outcome {
        LinkAddOutcome::Added => 0,
        LinkAddOutcome::Unreachable => 1,
        LinkAddOutcome::CertificateMismatch => 2,
    };
    let payload = serde_json::json!({
        "outcome": code,
        "snapshot": snap.as_ref().map(snapshot_to_dto),
    });
    new_jstring(&env, &payload.to_string())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerAddInvitedComputer(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    dto_json: JString,
    internet_play_reply: JString,
) -> jboolean {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return JNI_FALSE;
    };
    let (Some(raw), Some(reply)) = (jstr(&mut env, &dto_json), jstr(&mut env, &internet_play_reply))
    else {
        return JNI_FALSE;
    };
    let Ok(dto) = serde_json::from_str::<JniHostInputDto>(&raw) else {
        return JNI_FALSE;
    };
    jb(bridge
        .manager
        .add_invited_computer(input_to_entry(dto), &reply)
        .unwrap_or(false))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerInviteClaimsKnownPc(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    link_fp_sha256: JByteArray,
) -> jboolean {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return JNI_FALSE;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return JNI_FALSE;
    };
    let Some(sha_vec) = jbytes(&env, &link_fp_sha256) else {
        return JNI_FALSE;
    };
    let Ok(sha) = <[u8; 32]>::try_from(sha_vec) else {
        return JNI_FALSE;
    };
    jb(bridge.manager.invite_claims_known_pc(&uuid, sha))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerRemoveComputer(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let _ = bridge.manager.remove_computer(&uuid);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerPinAddress(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    address: JString,
    port: jint,
) {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let slot = jstr(&mut env, &address)
        .filter(|a| !a.is_empty() && port > 0)
        .and_then(|a| EndpointSlot::new(&a, port.min(u16::MAX as i32) as u16));
    let _ = bridge.manager.pin_address(&uuid, slot);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerForgetCertificate(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let _ = bridge.manager.forget_certificate(&uuid);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerSetServerCertificate(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    cert_der: JByteArray,
) {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let pem = jbytes(&env, &cert_der).map(|b| nightjar_core::http::pem_encode_certificate(&b));
    let _ = bridge
        .manager
        .set_server_certificate_pem(&uuid, pem.as_deref());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerRenameComputer(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    alias: JString,
) {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let alias = jstr(&mut env, &alias);
    let _ = bridge.manager.rename_computer(&uuid, alias.as_deref());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerResetNetworkState(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    available: jboolean,
    at_ms: jlong,
) {
    if let Some(bridge) = HOST_MANAGERS.get(handle) {
        bridge
            .manager
            .reset_network_state(available != 0, at_ms.max(0) as u64);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerInvalidateState(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return;
    };
    if let Some(uuid) = jstr(&mut env, &uuid) {
        bridge.manager.invalidate_state(&uuid);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerSetVpnActive(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    active: jboolean,
) {
    if let Some(bridge) = HOST_MANAGERS.get(handle) {
        bridge.manager.set_vpn_active(active != 0);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerSetRemoteEndpoint(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    address: JString,
    port: jint,
) {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let slot = jstr(&mut env, &address)
        .filter(|a| !a.is_empty() && port > 0)
        .and_then(|a| EndpointSlot::new(&a, port.min(u16::MAX as i32) as u16));
    let _ = bridge.manager.set_remote_endpoint(&uuid, slot);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerImportInternetPlay(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    reply_json: JString,
) {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let reply = jstr(&mut env, &reply_json).filter(|s| !s.is_empty());
    let _ = bridge
        .manager
        .import_internet_play(&uuid, reply.as_deref());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerPollDetails(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    dto_json: JString,
    state: JString,
    active_address: JString,
    active_port: jint,
) -> jstring {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return ptr::null_mut();
    };
    let Some(raw) = jstr(&mut env, &dto_json) else {
        return ptr::null_mut();
    };
    let Ok(dto) = serde_json::from_str::<JniHostInputDto>(&raw) else {
        return ptr::null_mut();
    };
    let reach = match jstr(&mut env, &state).as_deref() {
        Some("ONLINE") => HostReachState::Online,
        Some("OFFLINE") => HostReachState::Offline,
        _ => HostReachState::Unknown,
    };
    let active = jstr(&mut env, &active_address)
        .filter(|a| !a.is_empty() && active_port > 0)
        .and_then(|a| EndpointSlot::new(&a, active_port.min(u16::MAX as i32) as u16));
    let out = bridge
        .manager
        .poll_details(input_to_entry(dto), reach, active);
    let json = serde_json::json!({
        "ok": out.ok,
        "should_clear_cert": out.should_clear_cert,
        "snapshot": snapshot_to_dto(&out.snapshot),
    })
    .to_string();
    new_jstring(&env, &json)
}
