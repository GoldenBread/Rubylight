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
use nightjar_core::hosts::manager::{HostManager, HostSnapshot, LinkAddOutcome, refresh_external_ipv4_direct};
use nightjar_core::hosts::poll::{PollIpSuccess, is_site_local_host, parallel_poll_direct, try_poll_ip};
use nightjar_core::hosts::client_pool::HostClientPool;
use nightjar_core::http::{ClientIdentity, InfoPath};
use nightjar_policy::hosts::{CandidateSlot, EndpointSlot, HostReachState, HostSlots, ProbeCandidate, probe_plan};
use serde::Serialize;

use crate::http::new_jstring;
use crate::slab::{Slab, lock};

use super::{JniHostDto, base64_encode, dto_to_entry, entry_to_dto, jstr};

pub(crate) struct ManagedHostBridge {
    pub(crate) manager: HostManager,
    events: Arc<(Mutex<VecDeque<HostSnapshot>>, Condvar)>,
    closed: Arc<AtomicBool>,
}

impl ManagedHostBridge {
    fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        self.events.1.notify_all();
        self.manager.stop_polling();
    }
}

pub(crate) static HOST_MANAGERS: Slab<ManagedHostBridge> = Slab::new();

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
    }
}

#[derive(Serialize)]
struct JniPollIpDto {
    reached: bool,
    another_certificate: bool,
    should_clear_cert: bool,
    paired: bool,
    plain_path: bool,
    uuid: Option<String>,
    name: Option<String>,
    mac: Option<String>,
    https_port: u16,
    external_port: u16,
    local_ip: Option<String>,
    external_ip: Option<String>,
    current_game: i32,
    current_game_uuid: Option<String>,
    capabilities_json: Option<String>,
    raw_server_info: Option<String>,
    active: Option<EndpointSlot>,
    error: Option<String>,
}

fn success_to_poll_dto(win: &PollIpSuccess) -> JniPollIpDto {
    JniPollIpDto {
        reached: true,
        another_certificate: false,
        should_clear_cert: win.should_clear_cert,
        paired: win.info.paired,
        plain_path: matches!(win.path, InfoPath::Plain),
        uuid: Some(win.info.unique_id.clone()),
        name: Some(win.info.hostname.clone()),
        mac: win.info.mac_address.clone(),
        https_port: win.info.https_port,
        external_port: win.info.external_port,
        local_ip: win.info.local_ip.clone(),
        external_ip: win.info.external_ip.clone(),
        current_game: win.info.current_game,
        current_game_uuid: win.info.current_game_uuid.clone(),
        capabilities_json: win.info.capabilities.to_versioned_json(),
        raw_server_info: Some(win.info.raw.clone()),
        active: Some(EndpointSlot {
            address: win.candidate.address.clone(),
            port: win.candidate.port,
        }),
        error: None,
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
    if let Some(bridge) = HOST_MANAGERS.take(handle) {
        bridge.shutdown();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerResetForTests(
    _: JNIEnv,
    _: JClass,
) {
    for bridge in HOST_MANAGERS.drain_all() {
        bridge.shutdown();
    }
    super::hosts_poller::drain_all_pollers();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerSetIdentity(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    unique_id: JString,
    cert_pem: JString,
    key_pem: JString,
) {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return;
    };
    let (Some(uid), Some(cert), Some(key)) = (
        jstr(&mut env, &unique_id),
        jstr(&mut env, &cert_pem),
        jstr(&mut env, &key_pem),
    ) else {
        return;
    };
    if let Ok(identity) = ClientIdentity::from_pem(&uid, &cert, &key) {
        bridge.manager.pool().set_identity(Arc::new(identity));
    }
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
    }
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
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerPollHost(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    force: jboolean,
) -> jboolean {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return JNI_FALSE;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return JNI_FALSE;
    };
    jb(bridge.manager.poll_host(&uuid, force != 0))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerUpsertTuple(
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
    let Ok(dto) = serde_json::from_str::<JniHostDto>(&raw) else {
        return ptr::null_mut();
    };
    let snap = bridge.manager.upsert_tuple(dto_to_entry(dto));
    let json = serde_json::to_string(&snapshot_to_dto(&snap)).unwrap_or_default();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerSnapshots(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return new_jstring(&env, "[]");
    };
    let dtos: Vec<JniSnapshotDto> = bridge
        .manager
        .snapshots()
        .iter()
        .map(snapshot_to_dto)
        .collect();
    let json = serde_json::to_string(&dtos).unwrap_or_else(|_| "[]".into());
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
    let Ok(dto) = serde_json::from_str::<JniHostDto>(&raw) else {
        return ptr::null_mut();
    };
    let Some(snap) = bridge.manager.add_computer_blocking(dto_to_entry(dto)) else {
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
) -> jint {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return 1;
    };
    let Some(raw) = jstr(&mut env, &dto_json) else {
        return 1;
    };
    let Ok(dto) = serde_json::from_str::<JniHostDto>(&raw) else {
        return 1;
    };
    let Some(sha_vec) = jbytes(&env, &link_cert_sha256) else {
        return 1;
    };
    let Ok(sha) = <[u8; 32]>::try_from(sha_vec) else {
        return 1;
    };
    match bridge.manager.add_computer_for_link(dto_to_entry(dto), sha) {
        LinkAddOutcome::Added => 0,
        LinkAddOutcome::Unreachable => 1,
        LinkAddOutcome::CertificateMismatch => 2,
    }
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
    let Ok(dto) = serde_json::from_str::<JniHostDto>(&raw) else {
        return JNI_FALSE;
    };
    jb(bridge
        .manager
        .add_invited_computer(dto_to_entry(dto), &reply)
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
) {
    if let Some(bridge) = HOST_MANAGERS.get(handle) {
        bridge.manager.reset_network_state();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerClearPool(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(bridge) = HOST_MANAGERS.get(handle) {
        bridge.manager.pool().clear();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerIsSiteLocalHost(
    mut env: JNIEnv,
    _: JClass,
    host: JString,
) -> jboolean {
    let Some(host) = jstr(&mut env, &host) else {
        return JNI_FALSE;
    };
    jb(is_site_local_host(&host))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerRefreshExternalIpv4Direct(
    env: JNIEnv,
    _: JClass,
    external_port: jint,
) -> jstring {
    let Some(slot) = refresh_external_ipv4_direct(external_port.clamp(0, u16::MAX as i32) as u16)
    else {
        return ptr::null_mut();
    };
    new_jstring(&env, &serde_json::to_string(&slot).unwrap_or_default())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerPollIp(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    address: JString,
    port: jint,
    https_port: jint,
    cert_der: JByteArray,
    likely_online: jboolean,
) -> jstring {
    let Some(address) = jstr(&mut env, &address) else {
        return ptr::null_mut();
    };
    let uuid = jstr(&mut env, &uuid);
    let cert = jbytes(&env, &cert_der);
    let pool: Arc<HostClientPool> = match HOST_MANAGERS.get(handle) {
        Some(bridge) => Arc::clone(bridge.manager.pool()),
        None => {
            let Ok(id) = ClientIdentity::generate() else {
                return ptr::null_mut();
            };
            Arc::new(HostClientPool::new(Arc::new(id)))
        }
    };
    let candidate = ProbeCandidate {
        slot: CandidateSlot::Local,
        address,
        port: port.clamp(0, u16::MAX as i32) as u16,
        https_port: https_port.clamp(0, u16::MAX as i32) as u16,
    };
    let dto = match try_poll_ip(
        &pool,
        &candidate,
        uuid.as_deref(),
        cert.as_deref(),
        likely_online != 0,
    ) {
        Ok(win) => success_to_poll_dto(&win),
        Err(err) => JniPollIpDto {
            reached: false,
            another_certificate: matches!(err.kind, nightjar_policy::hosts::PollErrorKind::CertificateMismatch),
            should_clear_cert: false,
            paired: false,
            plain_path: false,
            uuid: None,
            name: None,
            mac: None,
            https_port: 0,
            external_port: 0,
            local_ip: None,
            external_ip: None,
            current_game: 0,
            current_game_uuid: None,
            capabilities_json: None,
            raw_server_info: None,
            active: None,
            error: err.message,
        },
    };
    let _ = base64_encode(&[]);
    let json = serde_json::to_string(&dto).unwrap_or_default();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostManagerPollDirect(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    slots_json: JString,
    online: jboolean,
    active_port: jint,
    https_port: jint,
    cert_der: JByteArray,
) -> jstring {
    let Some(bridge) = HOST_MANAGERS.get(handle) else {
        return ptr::null_mut();
    };
    let uuid = jstr(&mut env, &uuid);
    let slots = jstr(&mut env, &slots_json)
        .and_then(|s| serde_json::from_str::<HostSlots>(&s).ok())
        .unwrap_or_default();
    let cert = jbytes(&env, &cert_der);
    let active = (active_port > 0 && active_port <= u16::MAX as i32).then_some(active_port as u16);
    let plan = probe_plan(
        &slots,
        online != 0,
        active,
        https_port.clamp(0, u16::MAX as i32) as u16,
    );
    let outcome = parallel_poll_direct(
        bridge.manager.pool(),
        &plan,
        uuid.as_deref(),
        cert.as_deref(),
        online != 0,
    );
    let dto = match outcome.winner {
        Some(win) => success_to_poll_dto(&win),
        None => JniPollIpDto {
            reached: false,
            another_certificate: outcome.another_certificate,
            should_clear_cert: false,
            paired: false,
            plain_path: false,
            uuid: None,
            name: None,
            mac: None,
            https_port: 0,
            external_port: 0,
            local_ip: None,
            external_ip: None,
            current_game: 0,
            current_game_uuid: None,
            capabilities_json: None,
            raw_server_info: None,
            active: None,
            error: outcome.last_error.map(|e| format!("{e:?}")),
        },
    };
    new_jstring(&env, &serde_json::to_string(&dto).unwrap_or_default())
}
