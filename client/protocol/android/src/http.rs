//! JNI bridge for `com.limelight.nvstream.jni.NightjarHttpJni` (HTTP, TLS, pairing, launch, probes).
//!
//! Stores `Arc<HostClient>` and `Arc<TransferCancel>` in lock-free-outer [`Slab`] tables so
//! `cancelPendingRequests` never blocks behind an in-flight call. Every reply crosses JNI as a
//! canonical `{"ok": ...}` / `{"error": {"kind", "message", ...}}` JSON envelope.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jbyteArray, jint, jlong, jstring};
use nightjar_core::http::{
    App, AppControl, AppRole, ClientIdentity, DisplayLuminance, Error, Evidence, HostCapabilities,
    HostClient, HostDisplay, InfoPath, LaunchRequest, LaunchUnanswered, PairFailure, PairResult,
    ServerInfo, TransferCancel, asks_to_confirm_replace, asks_to_confirm_terminate,
    is_role_completed, is_terminate_completed, parse_app_list, random_ri_key,
};
use nightjar_core::sdp::AudioConfiguration;
use rubylight_protocol::fec_report::FecChoice;
use serde::Serialize;
use serde_json::{Value, json};

use crate::native_api;
use crate::slab::Slab;

pub(crate) static CLIENTS: Slab<HostClient> = Slab::new();
pub(crate) static CANCELS: Slab<TransferCancel> = Slab::new();

pub(crate) fn jbool(v: bool) -> jboolean {
    if v { JNI_TRUE } else { JNI_FALSE }
}

pub(crate) fn new_jstring(env: &JNIEnv, text: &str) -> jstring {
    match env.new_string(text) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

pub(crate) fn ok_json<T: Serialize>(value: T) -> String {
    json!({"ok": value}).to_string()
}

pub(crate) fn error_json(error: &Error) -> String {
    let kind = match error {
        Error::Io(_) => "network",
        Error::Tls(_) => "tls",
        Error::CertificateMismatch => "certificate_mismatch",
        Error::Status { .. } => "status",
        Error::NotFound(_) => "not_found",
        Error::Api { .. } => "api",
        Error::Malformed(_) => "malformed",
        Error::NotPaired => "not_paired",
        Error::InvalidIdentity(_) => "identity",
        Error::InvalidArgument(_) => "invalid_argument",
        Error::Cancelled => "cancelled",
    };
    let mut body = json!({
        "kind": kind,
        "message": error.to_string(),
    });
    if let Some(status) = error.http_status() {
        body["http_status"] = json!(status);
    }
    if let Error::Status { code, message } = error {
        body["error_code"] = json!(code);
        body["error_message"] = json!(message);
    }
    if let Some(api) = error.api_error() {
        body["code"] = serde_json::to_value(api.code).unwrap_or(Value::Null);
        body["setting"] = json!(api.setting);
        body["retry_after_ms"] = json!(api.retry_after_ms);
    }
    json!({"error": body}).to_string()
}

pub(crate) fn server_info_json(info: &ServerInfo) -> Value {
    json!({
        "raw": info.raw,
        "hostname": info.hostname,
        "unique_id": info.unique_id,
        "app_version": info.app_version,
        "gfe_version": info.gfe_version,
        "gpu_type": info.gpu_type,
        "https_port": info.https_port,
        "external_port": info.external_port,
        "mac_address": info.mac_address,
        "local_ip": info.local_ip,
        "external_ip": info.external_ip,
        "paired": info.paired,
        "state": info.state,
        "session_idle": info.session_idle,
        "current_game": info.current_game,
        "current_game_uuid": info.current_game_uuid,
        "server_codec_mode_support": info.server_codec_mode_support,
        "max_luma_pixels_h264": info.max_luma_pixels_h264,
        "max_luma_pixels_hevc": info.max_luma_pixels_hevc,
        "permission": info.permission,
        "server_commands": info.server_commands,
        "encoder_bitrate_supported": info.encoder_bitrate_supported,
        "encoder_bitrate_kbps": info.encoder_bitrate_kbps,
        "initial_encoder_bitrate_kbps": info.initial_encoder_bitrate_kbps,
        "frame_limiter_enabled": info.frame_limiter_enabled,
        "virtual_display_frame_limiter_enabled": info.virtual_display_frame_limiter_enabled,
        "frame_limiter_fps_limit_millihz": info.frame_limiter_fps_limit_millihz,
        "pyrowave_host_link_mbps": info.pyrowave_host_link_mbps,
        "pyrowave_bandwidth_probe_bytes": info.pyrowave_bandwidth_probe_bytes,
        "pyrowave_burst_probe": info.pyrowave_burst_probe,
        "pyrowave_critical_fec_percentage": info.pyrowave_critical_fec_percentage,
        "pyrowave_min_parity_shards": info.pyrowave_min_parity_shards,
        "packed444_modes": info.packed444_modes,
        "last_failure": info.last_failure,
        "capabilities": crate::capabilities::snapshot(&info.capabilities),
        "nvidia_server": info.nvidia_server(),
        "supports_4k": info.supports_4k(),
    })
}

pub(crate) fn caps_from_arg(env: &mut JNIEnv, caps_json: &JString) -> HostCapabilities {
    native_api::string(env, caps_json)
        .and_then(|s| Evidence::from_versioned_json(&s).map(HostCapabilities::from_evidence))
        .unwrap_or(HostCapabilities::UNKNOWN)
}

pub(crate) fn invalid_client() -> String {
    error_json(&Error::InvalidArgument("unknown HTTP client handle".into()))
}

fn decode_hex16(s: &str) -> Option<[u8; 16]> {
    let bytes = s.as_bytes();
    if bytes.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let hi = (pair[0] as char).to_digit(16)? as u8;
        let lo = (pair[1] as char).to_digit(16)? as u8;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

// --- Client & Cancel lifecycle ---

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientCreate(
    mut env: JNIEnv,
    _: JClass,
    address: JString,
    http_port: jint,
    https_port: jint,
    unique_id: JString,
    client_cert_der: JByteArray,
    client_key_pkcs8: JByteArray,
    server_cert_der: JByteArray,
    device_name: JString,
) -> jlong {
    let Some(address) = native_api::string(&mut env, &address) else {
        return 0;
    };
    let port = u16::try_from(http_port).unwrap_or(47989);
    let cert_der = native_api::bytes(&mut env, &client_cert_der).unwrap_or_default();
    let key_pkcs8 = native_api::bytes(&mut env, &client_key_pkcs8).unwrap_or_default();
    let uid = native_api::string(&mut env, &unique_id).unwrap_or_default();
    let client_res = if !cert_der.is_empty() && !key_pkcs8.is_empty() {
        ClientIdentity::from_der(uid, cert_der, key_pkcs8)
            .and_then(|id| HostClient::new(&address, port, Arc::new(id)))
    } else {
        HostClient::new_unauthenticated(&address, port, uid)
    };
    let Ok(mut client) = client_res else {
        return 0;
    };
    if let Some(name) = native_api::string(&mut env, &device_name)
        && !name.is_empty()
    {
        client = client.with_device_name(name);
    }
    if let Ok(hp) = u16::try_from(https_port)
        && hp > 0
    {
        client = client.with_https_port(hp);
    }
    if let Some(der) = native_api::bytes(&mut env, &server_cert_der)
        && !der.is_empty()
    {
        client.pin_server_certificate_der(Some(der));
    }
    if native_api::SECOND_SCREEN.load(Ordering::Relaxed) {
        client = client.with_second_screen();
    }
    CLIENTS.insert(client)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_setAppVersion(
    mut env: JNIEnv,
    _: JClass,
    version: JString,
) {
    if let Some(v) = native_api::string(&mut env, &version) {
        nightjar_core::http::set_app_version(&v);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    CLIENTS.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientCancelPendingRequests(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(client) = CLIENTS.get(handle) {
        client.cancel_pending_requests();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientSetHttpsPort(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    port: jint,
) {
    if let (Some(client), Ok(port)) = (CLIENTS.get(handle), u16::try_from(port)) {
        client.set_https_port(port);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientGetHttpsPort(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jint {
    CLIENTS
        .get(handle)
        .map(|c| i32::from(c.https_port_cached()))
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientSetServerCertDer(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    cert_der: JByteArray,
) {
    let Some(client) = CLIENTS.get(handle) else {
        return;
    };
    let der = native_api::bytes(&mut env, &cert_der).filter(|d| !d.is_empty());
    client.pin_server_certificate_der(der);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_clientGetServerCertDer(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jbyteArray {
    let Some(der) = CLIENTS.get(handle).and_then(|c| c.server_certificate_der()) else {
        return std::ptr::null_mut();
    };
    match env.byte_array_from_slice(&der) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_setSecondScreen(
    _: JNIEnv,
    _: JClass,
    second_screen: jboolean,
) {
    native_api::SECOND_SCREEN.store(second_screen != JNI_FALSE, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_transferCancelCreate(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    CANCELS.insert(TransferCancel::new())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_transferCancelDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    CANCELS.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_transferCancelCancel(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(c) = CANCELS.get(handle) {
        c.cancel();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_transferCancelIsCancelled(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jboolean {
    jbool(CANCELS.get(handle).is_some_and(|c| c.is_cancelled()))
}

// --- ServerInfo, Pairing, AppList ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getServerInfo(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    likely_online: jboolean,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.server_info_with_cert_check(likely_online != JNI_FALSE) {
            Ok((info, path)) => {
                let path_name = match path {
                    InfoPath::Pinned => "pinned",
                    InfoPath::Refused => "refused",
                    InfoPath::CertificateMismatch => "cert_mismatch",
                    InfoPath::Plain => "plain",
                };
                ok_json(json!({
                    "path": path_name,
                    "info": server_info_json(&info),
                }))
            }
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_probePairedComputer(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.server_info_https_strict(Duration::from_secs(3), 65_536) {
            Ok(info) => ok_json(server_info_json(&info)),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_parseServerInfoXml(
    mut env: JNIEnv,
    _: JClass,
    xml: JString,
    http_port: jint,
    strict: jboolean,
) -> jstring {
    let raw = native_api::string(&mut env, &xml).unwrap_or_default();
    let port = u16::try_from(http_port).unwrap_or(47989);
    let parsed = if strict != JNI_FALSE {
        ServerInfo::parse(&raw, port)
    } else {
        ServerInfo::parse_partial(&raw, port)
    };
    let reply = match parsed {
        Ok(info) => ok_json(server_info_json(&info)),
        Err(e) => error_json(&e),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_pair(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    server_info_xml: JString,
    pin: JString,
    passphrase: JString,
    expected_cert_sha256: JByteArray,
) -> jstring {
    let reply = (|| {
        let Some(client) = CLIENTS.get(handle) else {
            return invalid_client();
        };
        let raw_info = native_api::string(&mut env, &server_info_xml).unwrap_or_default();
        let info = match ServerInfo::parse(&raw_info, 47989) {
            Ok(i) => i,
            Err(e) => return error_json(&e),
        };
        let pin = native_api::string(&mut env, &pin).unwrap_or_default();
        let passphrase = native_api::string(&mut env, &passphrase);
        let expected_fp = native_api::bytes(&mut env, &expected_cert_sha256).and_then(|b| {
            let arr: [u8; 32] = b.try_into().ok()?;
            Some(arr)
        });
        let result = client.pair_checked(&info, &pin, passphrase.as_deref(), expected_fp);
        match result {
            Ok(PairResult::Paired {
                server_certificate_pem,
            }) => ok_json(json!({
                "state": "PAIRED",
                "server_certificate_pem": server_certificate_pem,
                "server_certificate_der": client.server_certificate_der(),
            })),
            Ok(PairResult::PinWrong) => ok_json(json!({"state": "PIN_WRONG"})),
            Ok(PairResult::AlreadyInProgress) => ok_json(json!({"state": "ALREADY_IN_PROGRESS"})),
            Ok(PairResult::Failed(PairFailure::ServerSignature)) => {
                ok_json(json!({"state": "SERVER_UNVERIFIED"}))
            }
            Ok(PairResult::Failed(PairFailure::Declined(_))) => ok_json(json!({"state": "FAILED"})),
            Err(Error::CertificateMismatch) => ok_json(json!({"state": "CERT_MISMATCH"})),
            Err(e) => error_json(&e),
        }
    })();
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_unpair(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.unpair() {
            Ok(()) => ok_json(true),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

fn app_json(app: &App) -> Value {
    let control = match app.control {
        AppControl::None => "NONE",
        AppControl::Resume => "RESUME",
        AppControl::DisconnectMonitor => "DISCONNECT_MONITOR",
        AppControl::DisconnectInput => "DISCONNECT_INPUT",
        AppControl::Terminate => "TERMINATE",
        AppControl::Monitor => "MONITOR",
        AppControl::Input => "INPUT",
        AppControl::RunningGame => "RUNNING_GAME",
    };
    json!({
        "name": app.name,
        "id": app.id,
        "hdr_supported": app.hdr_supported,
        "uuid": app.uuid,
        "host_index": app.host_index,
        "art_version": app.art_version,
        "control": control,
        "monitor_resume": app.monitor_resume,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getAppListRaw(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.app_list_raw() {
            Ok(raw) => ok_json(raw),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_parseAppListXml(
    mut env: JNIEnv,
    _: JClass,
    xml: JString,
    caps_json: JString,
) -> jstring {
    let raw = native_api::string(&mut env, &xml).unwrap_or_default();
    let caps = caps_from_arg(&mut env, &caps_json);
    let reply = match parse_app_list(&raw, &caps) {
        Ok(apps) => ok_json(apps.iter().map(app_json).collect::<Vec<_>>()),
        Err(e) => error_json(&e),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getBoxArt(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    app_id: jint,
) -> jbyteArray {
    let Some(client) = CLIENTS.get(handle) else {
        let _ = env.throw_new("java/io/IOException", "Unknown HTTP client");
        return std::ptr::null_mut();
    };
    match client.box_art(app_id) {
        Ok(bytes) => match env.byte_array_from_slice(&bytes) {
            Ok(arr) => arr.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        Err(Error::NotFound(_)) => std::ptr::null_mut(),
        Err(e) => {
            let _ = env.throw_new("java/io/IOException", e.to_string());
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getBoxArtUrl(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    app_id: jint,
) -> jstring {
    let Some(client) = CLIENTS.get(handle) else {
        let _ = env.throw_new("java/io/IOException", "Unknown HTTP client");
        return std::ptr::null_mut();
    };
    match client.box_art_url(app_id) {
        Ok(url) => new_jstring(&env, &url),
        Err(e) => {
            let _ = env.throw_new("java/io/IOException", e.to_string());
            std::ptr::null_mut()
        }
    }
}

// --- Launch, Quit, Role Disconnect, Bitrate ---

fn parse_launch_request(v: &Value) -> Result<LaunchRequest, Error> {
    let ri_key = decode_hex16(v["ri_key_hex"].as_str().unwrap_or_default())
        .ok_or_else(|| Error::InvalidArgument("bad 16-byte hex for ri_key_hex".into()))?;
    let role = match v["role"].as_str().unwrap_or("STREAM") {
        "REMOTE_MONITOR" => AppRole::RemoteMonitor,
        "INPUT_ONLY" => AppRole::InputOnly,
        _ => AppRole::Stream,
    };
    let host_display = match v["host_display"].as_str().unwrap_or("HOST_DEFAULT") {
        "EXCLUSIVE" => HostDisplay::Exclusive,
        "EXTENDED_PRIMARY" => HostDisplay::ExtendedPrimary,
        "EXTENDED" => HostDisplay::Extended,
        "EXTENDED_ISOLATED" => HostDisplay::ExtendedIsolated,
        "EXTENDED_PRIMARY_ISOLATED" => HostDisplay::ExtendedPrimaryIsolated,
        "PHYSICAL" => HostDisplay::Physical,
        _ => HostDisplay::HostDefault,
    };
    let audio = match v["audio_surround"].as_str().unwrap_or("STEREO") {
        "SURROUND_51" => AudioConfiguration::SURROUND_51,
        "SURROUND_71" => AudioConfiguration::SURROUND_71,
        "SURROUND_714" => AudioConfiguration::SURROUND_714,
        _ => AudioConfiguration::STEREO,
    };
    let fec = v["fec"].as_str().and_then(FecChoice::parse);
    let display_luminance = v["display_luminance"]
        .as_object()
        .map(|d| DisplayLuminance {
            max_nits: d["max_nits"].as_f64().unwrap_or(0.0) as f32,
            min_nits: d["min_nits"].as_f64().unwrap_or(0.0) as f32,
            max_full_frame_nits: d["max_full_frame_nits"].as_f64().unwrap_or(0.0) as f32,
            estimated_from_ratio: d["estimated_from_ratio"].as_bool().unwrap_or(false),
        });
    Ok(LaunchRequest {
        app_id: v["app_id"].as_i64().unwrap_or(0) as i32,
        app_uuid: v["app_uuid"].as_str().unwrap_or_default().to_owned(),
        role,
        small_stream_input_only: v["small_stream_input_only"].as_bool().unwrap_or(false),
        mic_only: v["mic_only"].as_bool().unwrap_or(false),
        width: v["width"].as_u64().unwrap_or(1920) as u32,
        height: v["height"].as_u64().unwrap_or(1080) as u32,
        fps: v["fps"].as_u64().unwrap_or(60) as u32,
        launch_refresh_rate_x100: v["launch_refresh_rate_x100"].as_u64().unwrap_or(0) as u32,
        client_refresh_rate_x100: v["client_refresh_rate_x100"].as_u64().unwrap_or(0) as u32,
        virtual_display: v["virtual_display"].as_bool().unwrap_or(false),
        virtual_display_scale: v["virtual_display_scale"].as_u64().unwrap_or(100) as u32,
        virtual_display_dpi: v["virtual_display_dpi"].as_u64().unwrap_or(0) as u32,
        host_display,
        vrr: v["vrr"].as_bool().unwrap_or(false),
        sops: v["sops"].as_bool().unwrap_or(true),
        play_local_audio: v["play_local_audio"].as_bool().unwrap_or(false),
        hdr: v["hdr"].as_bool().unwrap_or(false),
        ten_bit_sdr: v["ten_bit_sdr"].as_bool().unwrap_or(false),
        audio,
        ri_key,
        ri_key_id: v["ri_key_id"].as_i64().unwrap_or(0) as i32,
        prepared_controllers: v["prepared_controllers"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        gamepad_mask: v["gamepad_mask"].as_u64().unwrap_or(0) as u32,
        persist_gamepads: v["persist_gamepads"].as_bool().unwrap_or(false),
        fec,
        rate_change: v["rate_change"].as_bool().unwrap_or(false),
        confirm_replace: v["confirm_replace"].as_bool().unwrap_or(false),
        clipboard_sync: v["clipboard_sync"].as_bool().unwrap_or(false),
        display_luminance,
        rtx_hdr: v["rtx_hdr"].as_bool(),
        rtx_video: v["rtx_video"].as_bool(),
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_randomRiKey(
    env: JNIEnv,
    _: JClass,
) -> jbyteArray {
    let key = random_ri_key();
    match env.byte_array_from_slice(&key) {
        Ok(arr) => arr.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_buildLaunchQuery(
    mut env: JNIEnv,
    _: JClass,
    request_json: JString,
    caps_json: JString,
    nvidia: jboolean,
) -> jstring {
    let raw = native_api::string(&mut env, &request_json).unwrap_or_default();
    let caps = caps_from_arg(&mut env, &caps_json);
    let reply = match serde_json::from_str::<Value>(&raw)
        .map_err(|e| Error::InvalidArgument(e.to_string()))
        .and_then(|v| parse_launch_request(&v))
    {
        Ok(req) => ok_json(req.query(&caps, nvidia != JNI_FALSE)),
        Err(e) => error_json(&e),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_launchApp(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    verb: JString,
    request_json: JString,
    caps_json: JString,
    nvidia: jboolean,
    read_timeout_ms: jint,
) -> jstring {
    let reply = (|| {
        let Some(client) = CLIENTS.get(handle) else {
            return invalid_client();
        };
        let verb = native_api::string(&mut env, &verb).unwrap_or_else(|| "launch".into());
        let raw_req = native_api::string(&mut env, &request_json).unwrap_or_default();
        let caps = caps_from_arg(&mut env, &caps_json);
        let req = match serde_json::from_str::<Value>(&raw_req)
            .map_err(|e| Error::InvalidArgument(e.to_string()))
            .and_then(|v| parse_launch_request(&v))
        {
            Ok(r) => r,
            Err(e) => return error_json(&e),
        };
        let timeout =
            Duration::from_millis(u64::try_from(read_timeout_ms.max(1)).unwrap_or(120_000));
        match client.launch_with_capabilities(&verb, &req, &caps, nvidia != JNI_FALSE, timeout) {
            Ok(Some(launched)) => ok_json(json!({
                "ok": true,
                "session_url": launched.rtsp_session_url,
            })),
            Ok(None) => ok_json(json!({
                "ok": false,
            })),
            Err(Error::Status { code: 410, message }) => ok_json(json!({
                "ok": false,
                "confirming": true,
                "status": 410,
                "body": message,
            })),
            Err(Error::Io(ref io_err))
                if io_err
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<LaunchUnanswered>())
                    .is_some() =>
            {
                let seconds = io_err
                    .get_ref()
                    .and_then(|e| e.downcast_ref::<LaunchUnanswered>())
                    .map(|u| u.seconds)
                    .unwrap_or(1);
                json!({
                    "error": {
                        "kind": "launch_timeout",
                        "seconds": seconds,
                        "message": io_err.to_string(),
                    }
                })
                .to_string()
            }
            Err(e) => error_json(&e),
        }
    })();
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_disconnectRole(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    role_ordinal: jint,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => {
            let role = match role_ordinal {
                1 => AppRole::RemoteMonitor,
                2 => AppRole::InputOnly,
                _ => AppRole::Stream,
            };
            match client.disconnect_role(role) {
                Ok(v) => ok_json(v),
                Err(e) => error_json(&e),
            }
        }
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_quitApp(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.cancel() {
            Ok(v) => ok_json(v),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_launchConfirmationClassify(
    mut env: JNIEnv,
    _: JClass,
    status: jint,
    body: JString,
) -> jstring {
    let message = native_api::string(&mut env, &body).unwrap_or_default();
    let err = Error::Status {
        code: status,
        message,
    };
    let reply = ok_json(json!({
        "replace_required": asks_to_confirm_replace(&err),
        "terminate_required": asks_to_confirm_terminate(&err),
        "remote_monitor_completed": is_role_completed(AppRole::RemoteMonitor, &err),
        "input_only_completed": is_role_completed(AppRole::InputOnly, &err),
        "terminate_completed": is_terminate_completed(&err),
    }));
    new_jstring(&env, &reply)
}
