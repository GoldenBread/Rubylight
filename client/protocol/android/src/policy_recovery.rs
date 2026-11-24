//! JNI bridge for recovery, continue-here, exit, and input-only policies (`NightjarPolicyJni`).

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, jboolean, jint, jlong, jlongArray, jstring};
use nightjar_policy::continue_here;
use nightjar_policy::launch::{self, InputOnlyBlocker, InputOnlyMode, Role};
use nightjar_policy::recovery::{
    self, MAX_ATTEMPTS, StreamRecoveryPolicy, TRIP_RESUME_DELAY_MS, WINDOW_MS,
};

use crate::native_api;
use crate::policy::{jbool, non_neg_u32, non_neg_u64, shifted_u64};

fn jlong_sat(v: u64) -> jlong {
    v.min(i64::MAX as u64) as jlong
}

fn next_delay_values(
    attempts: jint,
    started_at_ms: jlong,
    now_ms: jlong,
    connected_at_ms: jlong,
) -> [i64; 3] {
    let mut policy = StreamRecoveryPolicy::new(non_neg_u32(attempts), non_neg_u64(started_at_ms));
    let delay = policy
        .next_delay(non_neg_u64(now_ms), connected_opt(connected_at_ms))
        .map(jlong_sat)
        .unwrap_or(-1);
    [
        delay,
        i64::from(policy.attempts()),
        jlong_sat(policy.started_at_ms()),
    ]
}

fn reset_after_stable_values(
    attempts: jint,
    started_at_ms: jlong,
    now_ms: jlong,
    connected_at_ms: jlong,
) -> [i64; 2] {
    let mut policy = StreamRecoveryPolicy::new(non_neg_u32(attempts), non_neg_u64(started_at_ms));
    policy.reset_after_stable_connection(non_neg_u64(now_ms), connected_opt(connected_at_ms));
    [
        i64::from(policy.attempts()),
        jlong_sat(policy.started_at_ms()),
    ]
}

fn after_user_trip_values(
    attempts: jint,
    started_at_ms: jlong,
    left_at_ms: jlong,
    now_ms: jlong,
) -> [i64; 2] {
    let shift = left_at_ms.min(now_ms).min(0).unsigned_abs();
    let policy = StreamRecoveryPolicy::after_user_trip(
        non_neg_u32(attempts),
        non_neg_u64(started_at_ms),
        shifted_u64(left_at_ms, shift),
        shifted_u64(now_ms, shift),
    );
    [
        i64::from(policy.attempts()),
        jlong_sat(policy.started_at_ms()),
    ]
}

pub(crate) fn long_array(env: &mut JNIEnv, values: &[i64]) -> jlongArray {
    let Ok(arr) = env.new_long_array(values.len() as i32) else {
        return std::ptr::null_mut();
    };
    if env.set_long_array_region(&arr, 0, values).is_err() {
        return std::ptr::null_mut();
    }
    arr.into_raw()
}

pub(crate) fn new_jstring(env: &JNIEnv, text: &str) -> jstring {
    match env.new_string(text) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn connected_opt(ms: jlong) -> Option<u64> {
    (ms >= 0).then_some(ms as u64)
}

fn role_from_ordinal(ordinal: jint) -> Role {
    match ordinal {
        1 => Role::RemoteMonitor,
        2 => Role::InputOnly,
        _ => Role::Stream,
    }
}

// --- Recovery (nightjar_policy::recovery, value-tuple JNI) ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryMaxAttempts(
    _: JNIEnv,
    _: JClass,
) -> jint {
    MAX_ATTEMPTS as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryWindowMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    WINDOW_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryTripResumeDelayMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    TRIP_RESUME_DELAY_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryNextDelay(
    mut env: JNIEnv,
    _: JClass,
    attempts: jint,
    started_at_ms: jlong,
    now_ms: jlong,
    connected_at_ms: jlong,
) -> jlongArray {
    let values = next_delay_values(attempts, started_at_ms, now_ms, connected_at_ms);
    long_array(&mut env, &values)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryResetAfterStableConnection(
    mut env: JNIEnv,
    _: JClass,
    attempts: jint,
    started_at_ms: jlong,
    now_ms: jlong,
    connected_at_ms: jlong,
) -> jlongArray {
    let values = reset_after_stable_values(attempts, started_at_ms, now_ms, connected_at_ms);
    long_array(&mut env, &values)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryAfterUserTrip(
    mut env: JNIEnv,
    _: JClass,
    attempts: jint,
    started_at_ms: jlong,
    left_at_ms: jlong,
    now_ms: jlong,
) -> jlongArray {
    let values = after_user_trip_values(attempts, started_at_ms, left_at_ms, now_ms);
    long_array(&mut env, &values)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryTripConnectedBefore(
    _: JNIEnv,
    _: JClass,
    attempts: jint,
    connected_for_ms: jlong,
) -> jlong {
    jlong_sat(recovery::trip_connected_before(
        non_neg_u32(attempts),
        non_neg_u64(connected_for_ms),
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryRecoversTermination(
    _: JNIEnv,
    _: JClass,
    code: jint,
    video_started: jboolean,
) -> jboolean {
    jbool(recovery::recovers_termination(
        code,
        video_started != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryTransientTermination(
    _: JNIEnv,
    _: JClass,
    code: jint,
) -> jboolean {
    jbool(recovery::transient_termination(code))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryTransportTermination(
    _: JNIEnv,
    _: JClass,
    code: jint,
) -> jboolean {
    jbool(recovery::transport_termination(code))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryTransientStartup(
    mut env: JNIEnv,
    _: JClass,
    stage: JString,
    port_flags: jint,
    error: jint,
) -> jboolean {
    let stage = native_api::string(&mut env, &stage);
    jbool(recovery::transient_startup(
        stage.as_deref(),
        port_flags,
        error,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryGaveUpCause(
    mut env: JNIEnv,
    _: JClass,
    last_failure: JString,
    unreachable: JString,
) -> jstring {
    let last = native_api::string(&mut env, &last_failure);
    let unreach = native_api::string(&mut env, &unreachable).unwrap_or_default();
    let cause = recovery::gave_up_cause(last.as_deref(), &unreach);
    new_jstring(&env, cause)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryGaveUpText(
    mut env: JNIEnv,
    _: JClass,
    cause: JString,
    stopped: JString,
) -> jstring {
    let c = native_api::string(&mut env, &cause).unwrap_or_default();
    let s = native_api::string(&mut env, &stopped).unwrap_or_default();
    new_jstring(&env, &recovery::gave_up_text(&c, &s))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryAutomaticBitrateStart(
    _: JNIEnv,
    _: JClass,
    encoder_kbps: jint,
    resume_kbps: jint,
) -> jint {
    let resume = (resume_kbps > 0).then_some(resume_kbps as u32);
    recovery::automatic_bitrate_start(non_neg_u32(encoder_kbps), resume) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryReconnectBitrateStart(
    _: JNIEnv,
    _: JClass,
    automatic: jboolean,
    automatic_owned: jboolean,
    manual: jboolean,
    kbps: jint,
) -> jint {
    recovery::reconnect_bitrate_start(
        automatic != JNI_FALSE,
        automatic_owned != JNI_FALSE,
        manual != JNI_FALSE,
        non_neg_u32(kbps),
    )
    .unwrap_or(0) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryVideoStarted(
    _: JNIEnv,
    _: JClass,
    decoder_received_video: jboolean,
    chain_video_shown: jboolean,
) -> jboolean {
    jbool(recovery::video_started(
        decoder_received_video != JNI_FALSE,
        chain_video_shown != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryIsAttempt(
    _: JNIEnv,
    _: JClass,
    attempts: jint,
    trip_resume: jboolean,
) -> jboolean {
    jbool(recovery::is_recovery_attempt(
        non_neg_u32(attempts),
        trip_resume != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryIsResumeOnly(
    _: JNIEnv,
    _: JClass,
    attempts: jint,
    trip_resume: jboolean,
    output_resume_only: jboolean,
) -> jboolean {
    jbool(recovery::is_resume_only_stream(
        non_neg_u32(attempts),
        trip_resume != JNI_FALSE,
        output_resume_only != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryHandOffInPlace(
    _: JNIEnv,
    _: JClass,
    task_root: jboolean,
    multi_window: jboolean,
    picture_in_picture: jboolean,
) -> jboolean {
    jbool(recovery::hand_off_in_place(
        task_root != JNI_FALSE,
        multi_window != JNI_FALSE,
        picture_in_picture != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryCanAutoRecover(
    _: JNIEnv,
    _: JClass,
    auto_reconnect: jboolean,
    displayed_failure: jboolean,
    connected: jboolean,
    foreground: jboolean,
    video_started: jboolean,
    finishing_or_destroyed: jboolean,
    handoff_or_pending: jboolean,
    stream_role: jboolean,
    control_action: jboolean,
    has_resume_target: jboolean,
    attempts: jint,
    started_at_ms: jlong,
    now_ms: jlong,
    connected_at_ms: jlong,
) -> jboolean {
    let policy = StreamRecoveryPolicy::new(non_neg_u32(attempts), non_neg_u64(started_at_ms));
    jbool(recovery::can_auto_recover(
        auto_reconnect != JNI_FALSE,
        displayed_failure != JNI_FALSE,
        connected != JNI_FALSE,
        foreground != JNI_FALSE,
        video_started != JNI_FALSE,
        finishing_or_destroyed != JNI_FALSE,
        handoff_or_pending != JNI_FALSE,
        stream_role != JNI_FALSE,
        control_action != JNI_FALSE,
        has_resume_target != JNI_FALSE,
        policy,
        non_neg_u64(now_ms),
        connected_opt(connected_at_ms),
    ))
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_recoveryCanRememberResume(
    _: JNIEnv,
    _: JClass,
    already_remembered: jboolean,
    auto_reconnect: jboolean,
    connected: jboolean,
    displayed_failure: jboolean,
    finishing_or_destroyed: jboolean,
    cancelled_on_pause: jboolean,
    handoff_in_progress: jboolean,
    recovery_app_id: jint,
    stream_role: jboolean,
    control_action: jboolean,
) -> jboolean {
    jbool(recovery::can_remember_resume(
        already_remembered != JNI_FALSE,
        auto_reconnect != JNI_FALSE,
        connected != JNI_FALSE,
        displayed_failure != JNI_FALSE,
        finishing_or_destroyed != JNI_FALSE,
        cancelled_on_pause != JNI_FALSE,
        handoff_in_progress != JNI_FALSE,
        recovery_app_id,
        stream_role != JNI_FALSE,
        control_action != JNI_FALSE,
    ))
}

// --- ContinueHere (nightjar_policy::continue_here) ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_continueHereAsksOnSelect(
    _: JNIEnv,
    _: JClass,
    offered: jboolean,
    plain_start: jboolean,
    same_game: jboolean,
) -> jboolean {
    jbool(continue_here::asks_on_select(
        offered != JNI_FALSE,
        plain_start != JNI_FALSE,
        same_game != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_continueHereReadsMoveAfterEnd(
    _: JNIEnv,
    _: JClass,
    offered: jboolean,
    left_on_purpose: jboolean,
    recovery_follows: jboolean,
) -> jboolean {
    jbool(continue_here::reads_move_after_end(
        offered != JNI_FALSE,
        left_on_purpose != JNI_FALSE,
        recovery_follows != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_continueHereRecovers(
    _: JNIEnv,
    _: JClass,
    transient_failure: jboolean,
    moved: jboolean,
) -> jboolean {
    jbool(continue_here::recovers(
        transient_failure != JNI_FALSE,
        moved != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_continueHereCameBack(
    _: JNIEnv,
    _: JClass,
    moved: jboolean,
    owns: jboolean,
) -> jboolean {
    jbool(continue_here::came_back(
        moved != JNI_FALSE,
        owns != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_continueHereJoinsAfterRead(
    _: JNIEnv,
    _: JClass,
    too: jboolean,
    same_game: jboolean,
    press_failed: jboolean,
) -> jboolean {
    jbool(continue_here::joins_after_read(
        too != JNI_FALSE,
        same_game != JNI_FALSE,
        press_failed != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_continueHereResumeRefusal(
    mut env: JNIEnv,
    _: JClass,
    join: jboolean,
    code: jint,
    message: JString,
) -> jint {
    let msg = native_api::string(&mut env, &message).unwrap_or_default();
    match continue_here::resume_refusal(join != JNI_FALSE, code, &msg) {
        None => 0,
        Some(continue_here::ResumeRefusal::SessionChanged) => 1,
        Some(continue_here::ResumeRefusal::BrowserStream) => 2,
    }
}

// --- Exit & InputOnly (nightjar_policy::launch) ---

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_exitQuitsOnLeave(
    _: JNIEnv,
    _: JClass,
    quit_on_leave_chosen: jboolean,
    role_ordinal: jint,
    connected: jboolean,
    second_screen: jboolean,
) -> jboolean {
    jbool(launch::quits_on_leave(
        quit_on_leave_chosen != JNI_FALSE,
        role_from_ordinal(role_ordinal),
        connected != JNI_FALSE,
        second_screen != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_exitTerminationNeedsAction(
    _: JNIEnv,
    _: JClass,
    error_code: jint,
    port_test_blocked: jboolean,
    network_grace_gave_up: jboolean,
    recovery_exhausted: jboolean,
) -> jboolean {
    jbool(launch::termination_needs_action(
        error_code,
        port_test_blocked != JNI_FALSE,
        network_grace_gave_up != JNI_FALSE,
        recovery_exhausted != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_exitHostEndedStream(
    _: JNIEnv,
    _: JClass,
    error_code: jint,
) -> jboolean {
    jbool(launch::host_ended_stream(error_code))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_exitRefusedInHandshake(
    mut env: JNIEnv,
    _: JClass,
    stage: JString,
) -> jboolean {
    let stage = native_api::string(&mut env, &stage);
    jbool(launch::refused_in_handshake(stage.as_deref()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_inputOnlyMode(
    _: JNIEnv,
    _: JClass,
    any_host_enabled: jboolean,
    remote_session_tiles: jboolean,
    host_lists_input_only: jboolean,
) -> jint {
    match launch::input_only_mode(
        any_host_enabled != JNI_FALSE,
        remote_session_tiles != JNI_FALSE,
        host_lists_input_only != JNI_FALSE,
    ) {
        InputOnlyMode::Native => 0,
        InputOnlyMode::SmallStream => 1,
        InputOnlyMode::Off => 2,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_inputOnlySmallStreamBlocker(
    _: JNIEnv,
    _: JClass,
    paired: jboolean,
    permission: jlong,
) -> jint {
    let perm = (permission >= 0).then_some(permission as u64);
    match launch::small_stream_blocker(paired != JNI_FALSE, perm) {
        None => 0,
        Some(InputOnlyBlocker::Pair) => 1,
        Some(InputOnlyBlocker::ViewPermission) => 2,
        Some(InputOnlyBlocker::InputPermission) => 3,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_inputOnlyKeepsRunningGame(
    _: JNIEnv,
    _: JClass,
    host_running_game: jboolean,
    this_game_running: jboolean,
) -> jboolean {
    jbool(launch::keeps_running_game(
        host_running_game != JNI_FALSE,
        this_game_running != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_inputOnlyEstimatedKbps(
    _: JNIEnv,
    _: JClass,
) -> jint {
    launch::estimated_kbps() as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_inputOnlyEstimatedMegabytesPerHour(
    _: JNIEnv,
    _: JClass,
) -> jint {
    launch::estimated_megabytes_per_hour() as jint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_and_exit_helpers_match_policy() {
        assert_eq!(connected_opt(-1), None);
        assert_eq!(connected_opt(0), Some(0));
        assert!(launch::termination_needs_action(-107, false, false, false));
        let mut p = StreamRecoveryPolicy::after_user_trip(2, 1_000, 20_000, 50_000);
        assert_eq!((p.attempts(), p.started_at_ms()), (2, 31_000));
        assert_eq!(p.next_delay(60_000, None), Some(4_000));
    }

    #[test]
    fn after_user_trip_saturates_large_started_at_without_wrapping_negative() {
        let res = after_user_trip_values(2, i64::MAX - 100, 1_000, 2_000);
        assert_eq!(res, [2, i64::MAX]);
        assert_eq!(next_delay_values(res[0] as jint, res[1], 3_000, -1)[0], -1);
    }

    #[test]
    fn after_user_trip_handles_negative_left_at_and_now() {
        assert_eq!(after_user_trip_values(2, 1_000, -500, 5_000), [2, 6_500]);
        assert_eq!(after_user_trip_values(2, 1_000, -5_000, -2_000), [2, 4_000]);
        assert_eq!(after_user_trip_values(2, 1_000, -2_000, -5_000), [2, 1_000]);
        assert_eq!(
            after_user_trip_values(2, 1_000, i64::MIN, i64::MAX),
            [2, i64::MAX]
        );
    }

    #[test]
    fn recovery_helpers_never_wrap_u64_to_negative_i64() {
        assert_eq!(jlong_sat(u64::MAX), i64::MAX);
        let next = next_delay_values(1, i64::MAX, i64::MAX, -1);
        assert!(next[2] >= 0);
        let reset = reset_after_stable_values(2, i64::MAX, 5_000, -1);
        assert_eq!(reset, [2, i64::MAX]);
        assert_eq!(
            jlong_sat(recovery::trip_connected_before(1, u64::MAX)),
            i64::MAX
        );
    }
}
