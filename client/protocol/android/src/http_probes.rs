//! JNI bridge for `NightjarHttpJni` bitrate, clipboard, metrics, route, internet-play,
//! Foundation ABR, and bandwidth/UDP/latency probes.

use std::io::Write;
use std::time::Duration;

use jni::JNIEnv;
use jni::objects::{JClass, JObject, JString, JValue};
use jni::sys::{jint, jlong, jstring};
use nightjar_core::http::foundation::{AbrAction, AbrConfigured, AbrFeedback, AbrMode, AbrRequest};
use nightjar_core::http::{
    Availability, Error, GpuMetrics, HardwareMetrics, HostApiError, ImageClipboardError,
    NetworkPath, Reason,
};
use serde_json::{Value, json};

use crate::http::{CLIENTS, caps_from_arg, error_json, invalid_client, new_jstring, ok_json};
use crate::native_api;

fn reason_name(reason: Reason) -> &'static str {
    match reason {
        Reason::UpdateHost => "UPDATE_HOST",
        Reason::Pair => "PAIR",
        Reason::ViewPermission => "VIEW_PERMISSION",
        Reason::ClipboardPermission => "CLIPBOARD_PERMISSION",
        Reason::CommandPermission => "COMMAND_PERMISSION",
        Reason::NoCommands => "NO_COMMANDS",
        Reason::VirtualDisplayOff => "VIRTUAL_DISPLAY_OFF",
        Reason::VirtualDisplayDriver => "VIRTUAL_DISPLAY_DRIVER",
        Reason::HardwareMetricsOff => "HARDWARE_METRICS_OFF",
        Reason::SleepPermission => "SLEEP_PERMISSION",
        Reason::RestartShutdownPermission => "RESTART_SHUTDOWN_PERMISSION",
        Reason::PairingApprovePermission => "PAIRING_APPROVE_PERMISSION",
        Reason::FilePermission => "FILE_PERMISSION",
        Reason::UsbPermission => "USB_PERMISSION",
        Reason::FolderPermission => "FOLDER_PERMISSION",
    }
}

fn host_api_error_json(error: &HostApiError) -> String {
    match error {
        HostApiError::UnsupportedVersion => json!({
            "error": {"kind": "unsupported_version", "message": "Unsupported response version"}
        })
        .to_string(),
        HostApiError::Unavailable {
            feature: _,
            availability,
        } => {
            let reason = match availability {
                Availability::Unsupported(Some(r)) => Some(reason_name(*r)),
                _ => None,
            };
            json!({
                "error": {
                    "kind": "unavailable",
                    "reason": reason,
                    "message": error.to_string(),
                }
            })
            .to_string()
        }
        HostApiError::Host(e) => error_json(e),
    }
}

// --- Bitrate, Clipboard, Image Clipboard, Metrics, Route, InternetPlay ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_setBitrate(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    bitrate_kbps: jint,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.set_bitrate(u32::try_from(bitrate_kbps.max(0)).unwrap_or(0)) {
            Ok(kbps) => ok_json(kbps),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getClipboard(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.clipboard() {
            Ok(text) => ok_json(text),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_sendClipboard(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    text: JString,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => {
            let text = native_api::string(&mut env, &text).unwrap_or_default();
            match client.send_clipboard(&text) {
                Ok(()) => ok_json(true),
                Err(e) => error_json(&e),
            }
        }
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

struct JniWriteSink<'local, 'env, 'obj> {
    env: &'env mut JNIEnv<'local>,
    sink: &'obj JObject<'local>,
    java_error: Option<String>,
}

impl Write for JniWriteSink<'_, '_, '_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let res = self.env.with_local_frame(4, |env| {
            let arr = env.byte_array_from_slice(buf)?;
            let arr_obj: JObject = arr.into();
            env.call_method(
                self.sink,
                "write",
                "([BII)V",
                &[
                    JValue::Object(&arr_obj),
                    JValue::Int(0),
                    JValue::Int(buf.len() as i32),
                ],
            )?;
            Ok::<(), jni::errors::Error>(())
        });
        if res.is_err() {
            if self.env.exception_check().unwrap_or(false) {
                let _ = self.env.exception_clear();
            }
            let msg = "Stream write failed".to_owned();
            self.java_error = Some(msg.clone());
            return Err(std::io::Error::other(msg));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn clipboard_image_error_json(err: ImageClipboardError) -> String {
    match err {
        ImageClipboardError::TooLarge(msg) => json!({
            "error": {"kind": "clipboard_rejected", "reason": "TOO_LARGE", "message": msg}
        })
        .to_string(),
        ImageClipboardError::NotPng(msg) => json!({
            "error": {"kind": "clipboard_rejected", "reason": "NOT_PNG", "message": msg}
        })
        .to_string(),
        ImageClipboardError::Unsupported => json!({
            "error": {"kind": "clipboard_rejected", "reason": "UNSUPPORTED", "message": err.to_string()}
        })
        .to_string(),
        ImageClipboardError::Forbidden => json!({
            "error": {"kind": "clipboard_rejected", "reason": "FORBIDDEN", "message": err.to_string()}
        })
        .to_string(),
        ImageClipboardError::Host(e) => error_json(&e),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getClipboardImage(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    sink_obj: JObject,
) -> jstring {
    get_clipboard_image(env, handle, sink_obj)
}

fn get_clipboard_image<'local>(
    mut env: JNIEnv<'local>,
    handle: jlong,
    sink_obj: JObject<'local>,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => {
            let mut sink = JniWriteSink {
                env: &mut env,
                sink: &sink_obj,
                java_error: None,
            };
            match client.clipboard_image_stream(&mut sink) {
                Ok(written) => ok_json(written),
                Err(e) => {
                    if let Some(local_msg) = sink.java_error {
                        json!({"error": {"kind": "network", "message": local_msg}}).to_string()
                    } else {
                        clipboard_image_error_json(e)
                    }
                }
            }
        }
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_sendClipboardImageFile(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    path: JString,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => {
            let path = native_api::string(&mut env, &path).unwrap_or_default();
            match client.send_clipboard_image_file(std::path::Path::new(&path)) {
                Ok(()) => ok_json(true),
                Err(e) => clipboard_image_error_json(e),
            }
        }
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

fn metrics_json(m: &HardwareMetrics) -> Value {
    let cpu: serde_json::Map<String, Value> = m
        .cpu
        .iter()
        .map(|(k, v)| (k.to_owned(), json!(v)))
        .collect();
    let gpus: Vec<Value> = m
        .gpus
        .iter()
        .map(|g: &GpuMetrics| {
            let readings: serde_json::Map<String, Value> = g
                .metrics
                .iter()
                .map(|(k, v)| (k.to_owned(), json!(v)))
                .collect();
            json!({
                "id": g.id,
                "name": g.name,
                "metrics": Value::Object(readings),
            })
        })
        .collect();
    json!({
        "sampled_at_unix_ms": m.sampled_at_unix_ms,
        "interval_ms": m.interval_ms,
        "cpu": Value::Object(cpu),
        "gpus": gpus,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getHardwareMetrics(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    caps_json: JString,
) -> jstring {
    let caps = caps_from_arg(&mut env, &caps_json);
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.hardware_metrics(&caps) {
            Ok(m) => ok_json(metrics_json(&m)),
            Err(e) => host_api_error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_parseHardwareMetrics(
    mut env: JNIEnv,
    _: JClass,
    raw_json: JString,
) -> jstring {
    let raw = native_api::string(&mut env, &raw_json).unwrap_or_default();
    let reply = match HardwareMetrics::parse(&raw) {
        Ok(m) => ok_json(metrics_json(&m)),
        Err(e) => host_api_error_json(&e),
    };
    new_jstring(&env, &reply)
}

fn network_path_name(path: NetworkPath) -> &'static str {
    match path {
        NetworkPath::Direct => "DIRECT",
        NetworkPath::Relay => "RELAY",
        NetworkPath::PeerRelay => "PEER_RELAY",
        NetworkPath::Unknown => "UNKNOWN",
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getNetworkPath(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    caps_json: JString,
) -> jstring {
    let caps = caps_from_arg(&mut env, &caps_json);
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.network_path(&caps) {
            Ok(p) => ok_json(network_path_name(p)),
            Err(e) => host_api_error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_parseNetworkPath(
    mut env: JNIEnv,
    _: JClass,
    raw_json: JString,
) -> jstring {
    let raw = native_api::string(&mut env, &raw_json).unwrap_or_default();
    let reply = match NetworkPath::parse(&raw) {
        Ok(p) => ok_json(network_path_name(p)),
        Err(e) => host_api_error_json(&e),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_getInternetPlay(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    caps_json: JString,
) -> jstring {
    let caps = caps_from_arg(&mut env, &caps_json);
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.internet_play(&caps) {
            Ok((raw, _)) => ok_json(raw),
            Err(e) => host_api_error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

// --- Foundation ABR & Bandwidth / Link / UDP / Latency Probes ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_supportsFoundationAbr(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.abr_capabilities() {
            Ok(caps) => ok_json(caps.supported),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_configureFoundationAbr(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    req_json: JString,
) -> jstring {
    let raw = native_api::string(&mut env, &req_json).unwrap_or_default();
    let reply = (|| {
        let Some(client) = CLIENTS.get(handle) else {
            return invalid_client();
        };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            return error_json(&Error::InvalidArgument("bad ABR config json".into()));
        };
        let mode = v["mode"].as_str().map(|m| match m {
            "quality" => AbrMode::Quality,
            "lowLatency" => AbrMode::LowLatency,
            _ => AbrMode::Balanced,
        });
        let req = AbrRequest {
            enabled: v["enabled"].as_bool().unwrap_or(false),
            mode,
            min_bitrate: v["min_bitrate"].as_u64().map(|n| n as u32),
            max_bitrate: v["max_bitrate"].as_u64().map(|n| n as u32),
        };
        match client.configure_abr(&req) {
            Ok(c) => {
                let c: AbrConfigured = c;
                ok_json(json!({
                    "success": c.success,
                    "enabled": c.enabled,
                    "initial_bitrate": c.initial_bitrate,
                    "min_bitrate": c.min_bitrate,
                    "max_bitrate": c.max_bitrate,
                    "host_max_bitrate": c.host_max_bitrate,
                    "max_bitrate_capped": c.max_bitrate_capped,
                    "bitrate_applied": c.bitrate_applied,
                    "bitrate_apply_error": c.bitrate_apply_error,
                    "error": c.error,
                }))
            }
            Err(e) => error_json(&e),
        }
    })();
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_sendFoundationAbrFeedback(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    fb_json: JString,
) -> jstring {
    let raw = native_api::string(&mut env, &fb_json).unwrap_or_default();
    let reply = (|| {
        let Some(client) = CLIENTS.get(handle) else {
            return invalid_client();
        };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            return error_json(&Error::InvalidArgument("bad ABR feedback json".into()));
        };
        let fb = AbrFeedback {
            packet_loss: v["packet_loss"].as_f64().unwrap_or(0.0),
            rtt_ms: v["rtt_ms"].as_f64().unwrap_or(0.0),
            decode_fps: v["decode_fps"].as_f64().unwrap_or(0.0),
            dropped_frames: v["dropped_frames"].as_u64().unwrap_or(0) as u32,
            current_bitrate: v["current_bitrate"].as_u64().unwrap_or(0) as u32,
        };
        match client.abr_feedback(&fb) {
            Ok(action) => {
                let action: AbrAction = action;
                ok_json(json!({
                    "new_bitrate": action.new_bitrate,
                    "bitrate_applied": action.bitrate_applied,
                    "bitrate_apply_error": action.bitrate_apply_error,
                    "reason": action.reason,
                }))
            }
            Err(e) => error_json(&e),
        }
    })();
    new_jstring(&env, &reply)
}

fn call_progress(env: &mut JNIEnv, listener: &JObject, pct: u8) {
    if listener.is_null() {
        return;
    }
    let called = env.with_local_frame(4, |env| {
        env.call_method(listener, "accept", "(I)V", &[JValue::Int(i32::from(pct))])
            .map(|_| ())
    });
    if called.is_err() && env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_probeBandwidth(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    bytes: jint,
    listener: JObject,
) -> jstring {
    probe_bandwidth_impl(env, handle, bytes, listener)
}

fn probe_bandwidth_impl<'local>(
    mut env: JNIEnv<'local>,
    handle: jlong,
    bytes: jint,
    listener: JObject<'local>,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => {
            let mut cb = |pct: u8| call_progress(&mut env, &listener, pct);
            let len = usize::try_from(bytes.max(0)).unwrap_or(0);
            match client.probe_bandwidth_raw_with_progress(len, &mut cb) {
                Ok(mbps) => ok_json(mbps),
                Err(e) => error_json(&e),
            }
        }
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_supportsFoundationBandwidthTest(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.supports_foundation_bandwidth_test() {
            Ok(v) => ok_json(v),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_probeFoundationBandwidth(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    listener: JObject,
) -> jstring {
    probe_foundation_bandwidth_impl(env, handle, listener)
}

fn probe_foundation_bandwidth_impl<'local>(
    mut env: JNIEnv<'local>,
    handle: jlong,
    listener: JObject<'local>,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => {
            let mut cb = |pct: u8| call_progress(&mut env, &listener, pct);
            match client.foundation_network_probe_with_progress(&mut cb) {
                Ok(res) => ok_json(json!({
                    "bandwidth_mbps": res.bandwidth_mbps,
                    "latency_ms": res.latency_ms,
                    "jitter_ms": res.jitter_ms,
                })),
                Err(e) => error_json(&e),
            }
        }
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_probeLinkMbps(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.probe_link_raw() {
            Ok(mbps) => ok_json(mbps),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_probePyroWaveUdp(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    kbps: jint,
    packet_size: jint,
    fps: jint,
    pace_kbps: jint,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => match client.probe_pace(
            u32::try_from(kbps.max(0)).unwrap_or(0),
            u32::try_from(packet_size.max(0)).unwrap_or(0),
            u32::try_from(fps.max(1)).unwrap_or(60),
            u32::try_from(pace_kbps.max(0)).unwrap_or(0),
        ) {
            Ok(m) => ok_json(json!({
                "expected": m.expected,
                "sent": m.sent,
                "received": m.received,
                "sender_ms": m.sender_ms,
                "loss": m.loss,
                "worst_window_loss": m.worst_window_loss,
                "frames": m.frames,
                "damaged_frames": m.damaged_frames,
                "excessive_loss_frames": m.excessive_loss_frames,
                "missing_tail_frames": m.missing_tail_frames,
                "worst_frame_loss": m.worst_frame_loss,
                "measured": m.measured(),
                "qualified": m.qualified(),
            })),
            Err(e) => error_json(&e),
        },
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_probeServerLatency(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    timeout_ms: jint,
) -> jstring {
    let reply = match CLIENTS.get(handle) {
        Some(client) => {
            let timeout = Duration::from_millis(u64::try_from(timeout_ms.max(1)).unwrap_or(1000));
            match client.probe_server_latency(timeout) {
                Ok(ms) => ok_json(ms),
                Err(e) => error_json(&e),
            }
        }
        None => invalid_client(),
    };
    new_jstring(&env, &reply)
}
