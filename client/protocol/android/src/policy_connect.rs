//! Stateless JNI policy exports for `NightjarSessionJni`, bridging
//! `nightjar_policy::{connect_text, mic_route, permissions}` to Android
//! (`NvConnection`, `GameConnection`).

use jni::JNIEnv;
use jni::objects::{JBooleanArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};
use nightjar_policy::connect_text::{
    LaunchFailureKind, StartupFailureKind, classify_launch_failure, classify_startup_failure,
    classify_termination, is_packed_444_refused, stage_key_from_name,
};
use nightjar_policy::launch::Role;
use nightjar_policy::mic_route::{
    ControllerMicState, MicRouteView, mic_disables_echo_cancellation, microphone_wanted,
    slice_controller_owns_route, slice_is_muted_for_reconnect, slice_route_view,
};
use nightjar_policy::permissions::{
    StreamPermissionContext, can_change_bitrate, can_disconnect_remote_session,
    can_quit_or_control_session, can_run_server_command, can_send_controller_input, can_send_input,
    server_command_throttle_allows,
};

use crate::hosts::jstr;
use crate::http::new_jstring;

fn jb(val: bool) -> jboolean {
    if val { JNI_TRUE } else { JNI_FALSE }
}

pub(crate) fn role_from_ordinal(ordinal: jint) -> Role {
    match ordinal {
        1 => Role::RemoteMonitor,
        2 => Role::InputOnly,
        _ => Role::Stream,
    }
}

fn read_bool_array(env: &JNIEnv, arr: &JBooleanArray) -> Vec<bool> {
    if arr.is_null() {
        return Vec::new();
    }
    let Ok(len) = env.get_array_length(arr) else {
        return Vec::new();
    };
    if len <= 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; len as usize];
    if env.get_boolean_array_region(arr, 0, &mut buf).is_err() {
        return Vec::new();
    }
    buf.into_iter().map(|b| b != 0).collect()
}

pub(crate) fn mic_route_snapshot_bits(mic_muted: bool, states: &[ControllerMicState]) -> i32 {
    let mut bits = 0i32;
    if slice_controller_owns_route(states) {
        bits |= 1;
    }
    if slice_is_muted_for_reconnect(mic_muted, states) {
        bits |= 2;
    }
    let view_code = match slice_route_view(mic_muted, states) {
        MicRouteView::Phone => 0,
        MicRouteView::ControllerMuted => 1,
        MicRouteView::ControllerReady { muted: false } => 2,
        MicRouteView::ControllerReady { muted: true } => 3,
    };
    bits | (view_code << 2)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_stageTextKey(
    mut env: JNIEnv,
    _: JClass,
    stage_name: JString,
) -> jstring {
    let name = jstr(&mut env, &stage_name);
    new_jstring(&env, stage_key_from_name(name.as_deref()).as_str())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_terminationTextKey(
    env: JNIEnv,
    _: JClass,
    code: jint,
) -> jstring {
    new_jstring(&env, classify_termination(code).tag())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_classifyStartupFailure(
    mut env: JNIEnv,
    _: JClass,
    has_decoder_failure: jboolean,
    stage_name: JString,
    has_connection_error_message: jboolean,
    error_code: jint,
) -> jint {
    let stage = jstr(&mut env, &stage_name);
    match classify_startup_failure(
        has_decoder_failure != 0,
        stage.as_deref(),
        has_connection_error_message != 0,
        error_code,
    ) {
        StartupFailureKind::DecoderFailure => 0,
        StartupFailureKind::ConnectionMessage => 1,
        StartupFailureKind::ConnectionFailed => 2,
        StartupFailureKind::HostError(_) => 3,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_classifyLaunchFailure(
    mut env: JNIEnv,
    _: JClass,
    is_local: jboolean,
    code: jint,
    trimmed_reason: JString,
) -> jint {
    let reason = jstr(&mut env, &trimmed_reason).unwrap_or_default();
    match classify_launch_failure(is_local != 0, code, &reason) {
        LaunchFailureKind::LocalOverride => 0,
        LaunchFailureKind::PermissionDenied { has_reason: false } => 1,
        LaunchFailureKind::PermissionDenied { has_reason: true } => 2,
        LaunchFailureKind::HostMessage { .. } => 3,
        LaunchFailureKind::HostError { .. } => 4,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_isPacked444Refused(
    _: JNIEnv,
    _: JClass,
    error_code: jint,
) -> jboolean {
    jb(is_packed_444_refused(error_code))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_micRouteSnapshot(
    env: JNIEnv,
    _: JClass,
    mic_muted: jboolean,
    allowed: JBooleanArray,
    ready: JBooleanArray,
    host_muted: JBooleanArray,
) -> jint {
    let a = read_bool_array(&env, &allowed);
    let r = read_bool_array(&env, &ready);
    let h = read_bool_array(&env, &host_muted);
    let count = a.len().min(r.len()).min(h.len());
    let states: Vec<ControllerMicState> = (0..count)
        .map(|i| ControllerMicState::new(a[i], r[i], h[i]))
        .collect();
    mic_route_snapshot_bits(mic_muted != 0, &states)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_microphoneWanted(
    _: JNIEnv,
    _: JClass,
    enable_mic: jboolean,
    mic_only: jboolean,
    remote_monitor: jboolean,
) -> jboolean {
    jb(microphone_wanted(
        enable_mic != 0,
        mic_only != 0,
        remote_monitor != 0,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_micDisablesEchoCancellation(
    _: JNIEnv,
    _: JClass,
    mic_only: jboolean,
) -> jboolean {
    jb(mic_disables_echo_cancellation(mic_only != 0))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_streamPermissions(
    _: JNIEnv,
    _: JClass,
    stop_requested: jboolean,
    is_monkey: jboolean,
    role_ordinal: jint,
    paired: jboolean,
    permission: jlong,
    supports_runtime_bitrate: jboolean,
) -> jint {
    let ctx = StreamPermissionContext {
        stop_requested: stop_requested != 0,
        is_monkey: is_monkey != 0,
        role: role_from_ordinal(role_ordinal),
        paired: paired != 0,
        permission,
    };
    let mut bits = 0i32;
    if can_send_input(&ctx) {
        bits |= 1;
    }
    if can_send_controller_input(&ctx) {
        bits |= 2;
    }
    if can_change_bitrate(&ctx, supports_runtime_bitrate != 0) {
        bits |= 4;
    }
    if can_quit_or_control_session(&ctx) {
        bits |= 8;
    }
    if can_disconnect_remote_session(&ctx) {
        bits |= 16;
    }
    bits
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_canRunServerCommand(
    _: JNIEnv,
    _: JClass,
    stop_requested: jboolean,
    is_monkey: jboolean,
    role_ordinal: jint,
    paired: jboolean,
    permission: jlong,
    command_index: jint,
    command_count: jint,
    command_matches_name: jboolean,
) -> jboolean {
    if command_index < 0 || command_count < 0 {
        return JNI_FALSE;
    }
    let ctx = StreamPermissionContext {
        stop_requested: stop_requested != 0,
        is_monkey: is_monkey != 0,
        role: role_from_ordinal(role_ordinal),
        paired: paired != 0,
        permission,
    };
    jb(can_run_server_command(
        &ctx,
        command_index as usize,
        command_count as usize,
        command_matches_name != 0,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_serverCommandThrottleAllows(
    _: JNIEnv,
    _: JClass,
    last_sent_ms: jlong,
    now_ms: jlong,
) -> jboolean {
    let prev = (last_sent_ms >= 0).then_some(last_sent_ms as u64);
    jb(server_command_throttle_allows(prev, now_ms.max(0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_snapshot_packs_ownership_mute_and_view() {
        assert_eq!(mic_route_snapshot_bits(false, &[]), 0);
        let ready_unmuted = [ControllerMicState::new(true, true, false)];
        assert_eq!(mic_route_snapshot_bits(false, &ready_unmuted), 1 | (2 << 2));
        let ready_muted = [ControllerMicState::new(true, true, false)];
        assert_eq!(
            mic_route_snapshot_bits(true, &ready_muted),
            1 | 2 | (3 << 2)
        );
        let host_muted = [ControllerMicState::new(false, false, true)];
        assert_eq!(
            mic_route_snapshot_bits(false, &host_muted),
            1 | 2 | (1 << 2)
        );
    }
}
