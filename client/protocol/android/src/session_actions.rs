//! Lifecycle, recovery gating, move-watch, and session-action JNI exports
//! for `NightjarSessionJni`, bridging `nightjar_core::connect::{actions, lifecycle}`
//! and `nightjar_policy::continue_here` to Android (`GameConnection`, `NvConnection`).

use std::sync::Mutex;

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jintArray, jlong, jstring};
use nightjar_core::connect::actions::serverinfo_confirms_ended_session;
use nightjar_core::connect::lifecycle::evaluate_termination;
use nightjar_policy::continue_here;
use nightjar_policy::http::{AppRole, PERMISSION_LAUNCH, PERMISSION_VIEW, ServerInfo};

use crate::connect::clear_metrics_pollers;
use crate::hosts::jstr;
use crate::http::new_jstring;
use crate::slab::Slab;

#[derive(Debug)]
pub(crate) struct MoveWatchState {
    remaining: u32,
}

static MOVE_WATCHES: Slab<Mutex<MoveWatchState>> = Slab::new();

fn jb(val: bool) -> jboolean {
    if val { JNI_TRUE } else { JNI_FALSE }
}

fn int_array(env: &JNIEnv, values: &[i32]) -> jintArray {
    let Ok(arr) = env.new_int_array(values.len() as i32) else {
        return std::ptr::null_mut();
    };
    if env.set_int_array_region(&arr, 0, values).is_err() {
        return std::ptr::null_mut();
    }
    arr.into_raw()
}

fn app_role_from_ordinal(ordinal: jint) -> AppRole {
    match ordinal {
        1 => AppRole::RemoteMonitor,
        2 => AppRole::InputOnly,
        _ => AppRole::Stream,
    }
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_sessionEvaluateTermination(
    env: JNIEnv,
    _: JClass,
    reported_code: jint,
    exit_requested: jboolean,
    host_gone: jboolean,
    ended_session: jboolean,
    moved_to_present: jboolean,
    guest_ended: jboolean,
    video_started: jboolean,
    auto_reconnect: jboolean,
) -> jintArray {
    let eval = evaluate_termination(
        reported_code,
        exit_requested != 0,
        host_gone != 0,
        ended_session != 0,
        moved_to_present != 0,
        guest_ended != 0,
        video_started != 0,
        auto_reconnect != 0,
    );
    int_array(
        &env,
        &[
            eval.effective_code,
            i32::from(eval.may_have_moved),
            i32::from(eval.recovery_follows),
            i32::from(eval.transient_failure),
            i32::from(eval.skip_port_test),
            i32::from(eval.keep_recovery_media_failure),
        ],
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_sessionStuckStopReport(
    mut env: JNIEnv,
    _: JClass,
    is_alive: jboolean,
    stack_lines: JString,
) -> jstring {
    if is_alive == 0 {
        return std::ptr::null_mut();
    }
    let lines = jstr(&mut env, &stack_lines).unwrap_or_default();
    let report = format!("Stream stop still running after 10 s:{lines}");
    new_jstring(&env, &report)
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_serverinfoConfirmsEndedSession(
    mut env: JNIEnv,
    _: JClass,
    stop_requested: jboolean,
    role_ordinal: jint,
    is_control_action: jboolean,
    paired: jboolean,
    has_view_launch_perm: jboolean,
    session_idle: jboolean,
    current_game: jint,
    current_game_uuid: JString,
) -> jboolean {
    let uuid = jstr(&mut env, &current_game_uuid).unwrap_or_default();
    let perm = if has_view_launch_perm != 0 {
        PERMISSION_VIEW | PERMISSION_LAUNCH
    } else {
        0
    };
    let info = ServerInfo::synthetic(
        paired != 0,
        current_game,
        &uuid,
        session_idle != 0,
        false,
        false,
        Some(perm),
    );
    jb(serverinfo_confirms_ended_session(
        stop_requested != 0,
        app_role_from_ordinal(role_ordinal),
        is_control_action != 0,
        &info,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_moveWatchCreate(
    _: JNIEnv,
    _: JClass,
    reads: jint,
) -> jlong {
    MOVE_WATCHES.insert(Mutex::new(MoveWatchState {
        remaining: reads.max(0) as u32,
    }))
}

pub(crate) fn step_move_watch(
    state: &mut MoveWatchState,
    showing: bool,
    moved_known: bool,
    moved: bool,
    owns: bool,
) -> i32 {
    if !showing || state.remaining == 0 {
        return 0;
    }
    if moved_known && continue_here::came_back(moved, owns) {
        state.remaining = 0;
        return 1;
    }
    state.remaining -= 1;
    if state.remaining == 0 { 0 } else { 2 }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_moveWatchStep(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    showing: jboolean,
    moved_known: jboolean,
    moved: jboolean,
    owns: jboolean,
) -> jint {
    let Some(lock) = MOVE_WATCHES.get(handle) else {
        return 0;
    };
    let mut state = lock.lock().unwrap_or_else(|p| p.into_inner());
    step_move_watch(
        &mut state,
        showing != 0,
        moved_known != 0,
        moved != 0,
        owns != 0,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_moveWatchDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    MOVE_WATCHES.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarSessionJni_sessionResetForTests(
    _: JNIEnv,
    _: JClass,
) {
    clear_metrics_pollers();
    MOVE_WATCHES.drain_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_watch_stops_on_came_back_or_exhaustion() {
        let mut s = MoveWatchState { remaining: 2 };
        assert_eq!(step_move_watch(&mut s, true, false, false, false), 2);
        assert_eq!(step_move_watch(&mut s, true, true, false, true), 1);

        let mut s2 = MoveWatchState { remaining: 2 };
        assert_eq!(step_move_watch(&mut s2, true, true, true, false), 2);
        assert_eq!(step_move_watch(&mut s2, true, true, true, false), 0);
    }
}
